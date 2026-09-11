//! Maintainer-only preparation and publication of the complete signed repository.
mod common;
mod engine;
mod github;
mod inventory;
mod remote;
mod stage;

use common::{Result, digest, read_regular, require};
use fs2::FileExt;
use std::{fs, path::Path};

fn publish(staging: &Path) -> Result<()> {
    let local_lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(staging.join("publisher.lock"))?;
    local_lock
        .try_lock_exclusive()
        .map_err(|_| "this staging directory is already being published")?;
    let mut state = stage::State::load(staging)?;
    state.verify(staging)?;
    let lock = remote::GitLock::new(&state.config);
    if state.published {
        if let Some(owner) = &state.owner {
            lock.release(owner)?;
        }
        println!("Publication already completed and verified.");
        return Ok(());
    }
    if state.owner.is_none() {
        state.owner = Some(lock.create_owner(&digest(&serde_json::to_vec(&state.assets)?))?);
        state.save(staging)?;
    }
    let owner = state.owner.clone().ok_or("missing lock owner")?;
    lock.acquire(&owner)?;
    require(
        github::is_draft(&state, staging)? || state.committing,
        "the target release is already published",
    )?;
    let mut remote = remote::R2::new(&state.config, staging)?;
    engine::check_baseline(
        &mut remote,
        &state.baseline,
        &state.assets,
        state.committing,
    )?;
    let read = |name: &str| read_regular(&staging.join("assets").join(name));
    engine::upload_immutable(&mut remote, &state.assets, read)?;
    engine::check_baseline(
        &mut remote,
        &state.baseline,
        &state.assets,
        state.committing,
    )?;
    state.committing = true;
    state.save(staging)?;
    engine::upload_mutable(&mut remote, &state.assets, read)?;
    engine::verify_public(&mut remote, &state.assets)?;
    // Identical hashes bind these public bytes to the signatures verified locally.
    github::synchronize(&state, staging)?;
    engine::verify_public(&mut remote, &state.assets)?;
    github::publish(&state, staging)?;
    state.published = true;
    state.save(staging)?;
    lock.release(&owner)?;
    println!(
        "Published {}: {} packages, {} verified artifacts.",
        state.config.tag,
        state.inventory.len(),
        state.assets.len()
    );
    Ok(())
}

fn execute() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("prepare") if args.len() == 5 => stage::prepare(Path::new(&args[2]), Path::new(&args[3]), Path::new(&args[4])),
        Some("publish") if args.len() == 3 => publish(&fs::canonicalize(&args[2])?),
        _ => Err("usage: t1-release prepare CONFIG.json CANDIDATE_DIR NEW_STAGE_DIR | t1-release publish STAGE_DIR".into()),
    }
}
fn main() {
    if let Err(error) = execute() {
        eprintln!("t1-release: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod verification_tests;
