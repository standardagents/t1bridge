use crate::common::{Config, Manifest, Result, digest, read_regular, require, run, safe_name};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Package {
    pub version: String,
    pub filename: String,
    pub sha256: String,
}
pub type Inventory = BTreeMap<String, Package>;

fn field<'a>(text: &'a str, field: &str) -> Result<&'a str> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line == field {
            return lines
                .next()
                .ok_or_else(|| format!("missing {field}").into());
        }
    }
    Err(format!("missing {field}").into())
}

pub fn index(directory: &Path, archive: &str) -> Result<Inventory> {
    let listing = run("bsdtar", &["-tf", archive], directory, None)?;
    let mut inventory = Inventory::new();
    for member in std::str::from_utf8(&listing)?
        .lines()
        .filter(|name| name.ends_with("/desc"))
    {
        require(
            member.split('/').count() == 2 && member.split('/').all(safe_name),
            "unsafe database member",
        )?;
        let bytes = run("bsdtar", &["-xOf", archive, member], directory, None)?;
        let desc = std::str::from_utf8(&bytes)?;
        let name = field(desc, "%NAME%")?;
        let package = Package {
            version: field(desc, "%VERSION%")?.into(),
            filename: field(desc, "%FILENAME%")?.into(),
            sha256: field(desc, "%SHA256SUM%")?.into(),
        };
        require(
            safe_name(name) && safe_name(&package.filename),
            "unsafe package metadata",
        )?;
        require(
            inventory.insert(name.into(), package).is_none(),
            "duplicate package in index",
        )?;
    }
    require(!inventory.is_empty(), "empty package index")?;
    Ok(inventory)
}

pub fn package(directory: &Path, filename: &str) -> Result<(String, Package)> {
    let bytes = run("bsdtar", &["-xOf", filename, ".PKGINFO"], directory, None)?;
    let info = std::str::from_utf8(&bytes)?;
    let value = |name: &str| -> Result<String> {
        let values: Vec<_> = info
            .lines()
            .filter_map(|line| line.strip_prefix(&format!("{name} = ")))
            .collect();
        require(values.len() == 1, format!("missing or duplicate {name}"))?;
        Ok(values[0].to_string())
    };
    let name = value("pkgname")?;
    let version = value("pkgver")?;
    require(
        safe_name(&name) && !version.is_empty(),
        "invalid package metadata",
    )?;
    Ok((
        name,
        Package {
            version,
            filename: filename.into(),
            sha256: digest(&read_regular(&directory.join(filename))?),
        },
    ))
}

pub fn merge(current: &Inventory, updates: &Inventory, directory: &Path) -> Result<Inventory> {
    let mut result = current.clone();
    for (name, new) in updates {
        if let Some(old) = current.get(name) {
            let comparison = run("vercmp", &[&new.version, &old.version], directory, None)?;
            let comparison: i32 = std::str::from_utf8(&comparison)?.trim().parse()?;
            require(comparison >= 0, format!("package downgrade: {name}"))?;
            require(
                comparison != 0 || old == new,
                format!("same version has different artifacts: {name}"),
            )?;
        }
        result.insert(name.clone(), new.clone());
    }
    Ok(result)
}

pub fn verify_repository(
    config: &Config,
    directory: &Path,
    manifest: &Manifest,
) -> Result<Inventory> {
    for (name, hash) in manifest {
        require(
            digest(&read_regular(&directory.join(name))?) == *hash,
            format!("checksum mismatch: {name}"),
        )?;
    }
    for name in ["standardagents.db", "standardagents.files"] {
        require(
            manifest.contains_key(name) && manifest.contains_key(&format!("{name}.sig")),
            "unsigned/missing index",
        )?;
        config.verify(directory, name)?;
    }
    let inventory = index(directory, "standardagents.db")?;
    require(
        index(directory, "standardagents.files")? == inventory,
        "database and files inventories differ",
    )?;
    for (name, entry) in &inventory {
        require(
            manifest.get(&entry.filename) == Some(&entry.sha256)
                && manifest.contains_key(&format!("{}.sig", entry.filename)),
            "package absent from signed manifest",
        )?;
        config.verify(directory, &entry.filename)?;
        require(
            package(directory, &entry.filename)? == (name.clone(), entry.clone()),
            "package disagrees with index",
        )?;
    }
    Ok(inventory)
}

pub fn build_indexes(config: &Config, directory: &Path, inventory: &Inventory) -> Result<()> {
    let mut args = vec!["--include-sigs", "standardagents.db.tar.zst"];
    args.extend(inventory.values().map(|entry| entry.filename.as_str()));
    run("repo-add", &args, directory, None)?;
    for kind in ["db", "files"] {
        let archive = format!("standardagents.{kind}.tar.zst");
        let alias = format!("standardagents.{kind}");
        // repo-add creates symlinks; GitHub/R2 need the actual bytes.
        std::fs::remove_file(directory.join(&alias))?;
        std::fs::copy(directory.join(&archive), directory.join(&alias))?;
        config.sign(directory, &archive)?;
        std::fs::copy(
            directory.join(format!("{archive}.sig")),
            directory.join(format!("{alias}.sig")),
        )?;
    }
    require(
        index(directory, "standardagents.db")? == *inventory
            && index(directory, "standardagents.files")? == *inventory,
        "generated indexes changed the intended inventory",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(version: &str) -> Package {
        Package {
            version: version.into(),
            filename: format!("sample-{version}.pkg.tar.zst"),
            sha256: version.into(),
        }
    }
    #[test]
    fn add_upgrade_preserves_unrelated_and_rejects_downgrade_or_rebuild() {
        let current: Inventory = [
            ("core".into(), entry("2-1")),
            ("optional".into(), entry("7-1")),
        ]
        .into();
        let updates = [("core".into(), entry("3-1")), ("new".into(), entry("1-1"))].into();
        let combined = merge(&current, &updates, Path::new(".")).unwrap();
        assert_eq!(combined.len(), 3);
        assert_eq!(combined["optional"], current["optional"]);
        assert_eq!(combined["core"].version, "3-1");
        assert!(merge(&combined, &current, Path::new(".")).is_err());
        let mut changed = combined.clone();
        changed.get_mut("core").unwrap().sha256 = "different".into();
        assert!(merge(&combined, &changed, Path::new(".")).is_err());
    }
}
