use crate::{
    common::{Result, digest, path_str, read_regular, require, run, safe_name},
    stage::State,
};
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Deserialize)]
struct Asset {
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    is_draft: bool,
    assets: Vec<Asset>,
}

fn release(state: &State, staging: &Path) -> Result<Release> {
    Ok(serde_json::from_slice(&run(
        "gh",
        &[
            "release",
            "view",
            &state.config.tag,
            "--repo",
            &state.config.repository,
            "--json",
            "isDraft,assets",
        ],
        staging,
        None,
    )?)?)
}

pub fn is_draft(state: &State, staging: &Path) -> Result<bool> {
    Ok(release(state, staging)?.is_draft)
}

pub fn synchronize(state: &State, staging: &Path) -> Result<()> {
    let release = release(state, staging)?;
    let downloads = staging.join("github");
    fs::create_dir_all(&downloads)?;
    // Published releases are only verified on resume, never edited.
    for (name, hash) in &state.assets {
        let exists = release.assets.iter().any(|asset| asset.name == *name);
        let mut matches = false;
        if exists {
            run(
                "gh",
                &[
                    "release",
                    "download",
                    &state.config.tag,
                    "--repo",
                    &state.config.repository,
                    "--pattern",
                    name,
                    "--dir",
                    path_str(&downloads)?,
                    "--clobber",
                ],
                staging,
                None,
            )?;
            matches = digest(&read_regular(&downloads.join(name))?) == *hash;
        }
        if !matches {
            require(
                release.is_draft,
                format!("published GitHub artifact differs/missing: {name}"),
            )?;
            println!("Synchronizing GitHub asset {name}");
            run(
                "gh",
                &[
                    "release",
                    "upload",
                    &state.config.tag,
                    "--repo",
                    &state.config.repository,
                    path_str(&staging.join("assets").join(name))?,
                    "--clobber",
                ],
                staging,
                None,
            )?;
            run(
                "gh",
                &[
                    "release",
                    "download",
                    &state.config.tag,
                    "--repo",
                    &state.config.repository,
                    "--pattern",
                    name,
                    "--dir",
                    path_str(&downloads)?,
                    "--clobber",
                ],
                staging,
                None,
            )?;
            require(
                digest(&read_regular(&downloads.join(name))?) == *hash,
                format!("GitHub download mismatch: {name}"),
            )?;
        }
    }
    for asset in release
        .assets
        .iter()
        .filter(|asset| !state.assets.contains_key(&asset.name))
    {
        require(
            release.is_draft && safe_name(&asset.name),
            "unexpected published GitHub asset",
        )?;
        run(
            "gh",
            &[
                "release",
                "delete-asset",
                &state.config.tag,
                &asset.name,
                "--repo",
                &state.config.repository,
                "--yes",
            ],
            staging,
            None,
        )?;
    }
    state.config.verify(&downloads, "SHA256SUMS")?;
    Ok(())
}

pub fn publish(state: &State, staging: &Path) -> Result<()> {
    if is_draft(state, staging)? {
        run(
            "gh",
            &[
                "release",
                "edit",
                &state.config.tag,
                "--repo",
                &state.config.repository,
                "--draft=false",
                "--prerelease",
                "--notes-file",
                path_str(&staging.join("notes.md"))?,
            ],
            staging,
            None,
        )?;
    }
    require(
        !is_draft(state, staging)?,
        "GitHub release is still a draft",
    )
}
