use crate::{
    common::{
        Config, Manifest, Result, atomic_json, digest, manifest_bytes, mutable, parse_manifest,
        read_regular, require, safe_name,
    },
    engine::{self, Store},
    inventory::{self, Inventory},
    remote::R2,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub config: Config,
    pub baseline: Manifest,
    pub assets: Manifest,
    pub inventory: Inventory,
    pub owner: Option<String>,
    pub committing: bool,
    pub published: bool,
    pub notes_hash: String,
}
impl State {
    pub fn save(&self, staging: &Path) -> Result<()> {
        atomic_json(&staging.join("state.json"), self)
    }
    pub fn load(staging: &Path) -> Result<Self> {
        let state: Self = serde_json::from_slice(&read_regular(&staging.join("state.json"))?)?;
        state.config.validate()?;
        require(
            state
                .assets
                .keys()
                .chain(state.baseline.keys())
                .all(|name| safe_name(name)),
            "unsafe staged filename",
        )?;
        Ok(state)
    }
    fn capture_unlisted_mutable(&mut self, remote: &mut impl Store) -> Result<()> {
        // Historical aliases are conflict guards, never trusted package inputs.
        for name in self.assets.keys().filter(|name| mutable(name)) {
            if !self.baseline.contains_key(name)
                && let Some(bytes) = remote.origin(name)?
            {
                self.baseline.insert(name.clone(), digest(&bytes));
            }
        }
        Ok(())
    }
    pub fn verify(&self, staging: &Path) -> Result<()> {
        let assets = staging.join("assets");
        require(
            hash_directory(&assets)? == self.assets,
            "staged artifacts changed; do not rebuild or modify an active staging",
        )?;
        require(
            digest(&read_regular(&staging.join("notes.md"))?) == self.notes_hash,
            "staged notes changed",
        )?;
        self.config.verify(&assets, "SHA256SUMS")?;
        let mut manifest = parse_manifest(&read_regular(&assets.join("SHA256SUMS"))?)?;
        let inventory = inventory::verify_repository(&self.config, &assets, &manifest)?;
        for name in ["SHA256SUMS", "SHA256SUMS.sig"] {
            manifest.insert(name.into(), self.assets[name].clone());
        }
        require(
            manifest == self.assets && inventory == self.inventory,
            "staged manifest/inventory mismatch",
        )
    }
}

fn generated(name: &str) -> bool {
    name.starts_with("standardagents.db")
        || name.starts_with("standardagents.files")
        || name.starts_with("SHA256SUMS")
}

pub fn hash_directory(directory: &Path) -> Result<Manifest> {
    let mut manifest = Manifest::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF8 filename")?;
        require(safe_name(&name), "unsafe artifact filename")?;
        manifest.insert(name, digest(&read_regular(&entry.path())?));
    }
    Ok(manifest)
}

pub fn prepare(config_path: &Path, candidate: &Path, staging: &Path) -> Result<()> {
    let config: Config = serde_json::from_slice(&read_regular(config_path)?)?;
    config.validate()?;
    require(
        !staging.exists(),
        "staging already exists; resume it or choose a new staging directory",
    )?;
    fs::create_dir(staging)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staging, fs::Permissions::from_mode(0o700))?;
    }
    let baseline_dir = staging.join("baseline");
    let assets = staging.join("assets");
    fs::create_dir(&baseline_dir)?;
    fs::create_dir(&assets)?;
    let mut remote = R2::new(&config, staging)?;
    let (current, baseline) = fetch_baseline(&config, &mut remote, &baseline_dir)?;
    let candidates = hash_directory(candidate)?;
    // CI's manifest is not signed as a whole, but its package signatures are.
    // Check the manifest when supplied, then verify each candidate package.
    if candidates.contains_key("SHA256SUMS") {
        for (name, hash) in parse_manifest(&read_regular(&candidate.join("SHA256SUMS"))?)? {
            require(
                candidates.get(&name) == Some(&hash),
                "candidate checksum mismatch",
            )?;
        }
    }
    let mut updates = Inventory::new();
    for name in candidates
        .keys()
        .filter(|name| name.ends_with(".pkg.tar.zst"))
    {
        config.verify(candidate, name)?;
        let (package_name, entry) = inventory::package(candidate, name)?;
        require(
            updates.insert(package_name, entry).is_none(),
            "duplicate candidate package",
        )?;
    }
    require(!updates.is_empty(), "candidate contains no signed packages")?;
    let inventory = inventory::merge(&current, &updates, candidate)?;
    for name in baseline.keys().filter(|name| !generated(name)) {
        if current.values().any(|entry| {
            (name == &entry.filename || name == &format!("{}.sig", entry.filename))
                && !inventory.values().any(|new| new == entry)
        }) {
            continue;
        }
        fs::copy(baseline_dir.join(name), assets.join(name))?;
    }
    for (name, hash) in candidates.iter().filter(|(name, _)| !generated(name)) {
        require(
            mutable(name) || baseline.get(name).is_none_or(|old| old == hash),
            format!("candidate changes immutable artifact: {name}"),
        )?;
        fs::copy(candidate.join(name), assets.join(name))?;
    }
    inventory::build_indexes(&config, &assets, &inventory)?;
    let unsigned = hash_directory(&assets)?
        .into_iter()
        .filter(|(name, _)| {
            Path::new(name)
                .extension()
                .is_none_or(|extension| extension != "sig")
        })
        .collect();
    fs::write(
        assets.join("SHA256SUMS.unsigned"),
        manifest_bytes(&unsigned),
    )?;
    fs::write(
        assets.join("SHA256SUMS"),
        manifest_bytes(&hash_directory(&assets)?),
    )?;
    config.sign(&assets, "SHA256SUMS")?;
    let notes = read_regular(Path::new(&config.notes_file))?;
    fs::write(staging.join("notes.md"), &notes)?;
    let mut state = State {
        config,
        baseline,
        assets: hash_directory(&assets)?,
        inventory,
        owner: None,
        committing: false,
        published: false,
        notes_hash: digest(&notes),
    };
    state.verify(staging)?;
    state.capture_unlisted_mutable(&mut remote)?;
    engine::check_baseline(&mut remote, &state.baseline, &state.assets, false)?;
    state.save(staging)?;
    println!(
        "Prepared {} packages and {} artifacts. Review state.json, assets and notes.md before publishing.",
        state.inventory.len(),
        state.assets.len()
    );
    for (name, entry) in &state.inventory {
        println!("  {name} {}", entry.version);
    }
    Ok(())
}

fn fetch_baseline(
    config: &Config,
    remote: &mut impl Store,
    baseline_dir: &Path,
) -> Result<(Inventory, Manifest)> {
    for name in ["SHA256SUMS", "SHA256SUMS.sig"] {
        let bytes = remote
            .origin(name)?
            .ok_or("current signed manifest is missing")?;
        fs::write(baseline_dir.join(name), bytes)?;
    }
    config.verify(baseline_dir, "SHA256SUMS")?;
    let baseline_manifest = parse_manifest(&fs::read(baseline_dir.join("SHA256SUMS"))?)?;
    for (name, hash) in &baseline_manifest {
        let bytes = remote
            .origin(name)?
            .ok_or_else(|| format!("current artifact is missing: {name}"))?;
        require(
            digest(&bytes) == *hash,
            format!("current artifact mismatch: {name}"),
        )?;
        fs::write(baseline_dir.join(name), bytes)?;
    }
    let current = inventory::verify_repository(config, baseline_dir, &baseline_manifest)?;
    let baseline = hash_directory(baseline_dir)?;
    Ok((current, baseline))
}
