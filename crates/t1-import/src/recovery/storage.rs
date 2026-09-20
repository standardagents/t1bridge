//! Recovery artifacts and EFI publication. A complete generation is staged and
//! flushed before one no-replace directory rename. Preserved data is never
//! replaced; interrupted attempts stay private and block accidental reuse.

use super::{Error, Result};
use std::{
    io::{self, Read, Write},
    os::fd::AsFd,
    path::Path,
};
use t1_platform::{
    preserved_efi_discovery,
    recovery_fs::{self, Directory},
};

const EXISTS: Error =
    Error("preserved EFI data exists; use local recovery or the selected same-Mac backup");

pub(super) fn require_local_absence() -> Result<()> {
    let roots = preserved_efi_discovery::discover_roots().map_err(|_| {
        Error("EFI inspection failed; absence of local data has not been established")
    })?;
    for root in roots {
        let apple = (|| -> io::Result<Directory> {
            recovery_fs::discovered_child(root.as_fd(), "EFI")?.child("APPLE", false, false)
        })();
        match apple {
            Ok(apple) => {
                if apple.exists("EMBEDDEDOS")? || apple.exists("EMBEDDEDOS.recovery")? {
                    return Err(EXISTS);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(Error(
                    "local EFI data could not be inspected; refusing online recovery",
                ));
            }
        }
    }
    Ok(())
}

pub(super) struct Attempt {
    root: Directory,
    active: Directory,
}

impl Attempt {
    pub fn create() -> Result<Self> {
        let parent =
            Directory::open(Path::new("/var/lib"), false)?.child("t1bridge", true, false)?;
        let root = parent.child("recovery", true, true)?;
        root.lock()
            .map_err(|_| Error("another recovery process holds the attempt lock"))?;
        if root.exists("active")? {
            return Err(Error(
                "an interrupted recovery is retained in /var/lib/t1bridge/recovery/active; inspect it before starting another attempt",
            ));
        }
        root.require_space(192 * 1024 * 1024)?;
        let active = root.child("active", true, true)?;
        save(&active, "started", b"native-recovery-v1\n")?;
        Ok(Self { root, active })
    }

    pub fn phase(&self, phase: &str) -> Result<Directory> {
        if self.active.exists(phase)? {
            return Err(Error("recovery phase already has artifacts"));
        }
        Ok(self.active.child(phase, true, true)?)
    }

    pub fn finish(self) -> Result<()> {
        save(&self.active, "complete", b"efi-generation-durable\n")?;
        let mut random = [0; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let suffix = u128::from_ne_bytes(random).to_string();
        self.root
            .rename("active", &self.root, &format!("complete-{suffix}"))?;
        self.root.sync()?;
        Ok(())
    }
}

pub(super) fn save(directory: &Directory, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = directory.file(name, true)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    directory.sync()?;
    // Read back from the held directory; a successful write is insufficient.
    let mut readback = Vec::new();
    directory
        .file(name, false)?
        .take(u64::try_from(bytes.len()).map_err(|_| Error("artifact exceeds size bound"))? + 1)
        .read_to_end(&mut readback)?;
    if readback != bytes {
        return Err(Error("recovery artifact failed readback verification"));
    }
    Ok(())
}

pub(super) struct EfiDestination {
    root: Directory,
}

impl EfiDestination {
    pub fn open(path: &Path) -> Result<Self> {
        let root = Directory::open(path, true).map_err(|_| Error("destination must be the root of an owner-controlled, mounted FAT EFI system partition"))?;
        root.lock()
            .map_err(|_| Error("another process holds the EFI recovery lock"))?;
        root.require_space(96 * 1024 * 1024)?;
        // Inspection is read-only until the attended restore has succeeded.
        match root
            .child("EFI", false, false)
            .and_then(|efi| efi.child("APPLE", false, false))
        {
            Ok(apple) => {
                if apple.exists("EMBEDDEDOS")? || apple.exists("EMBEDDEDOS.recovery")? {
                    return Err(EXISTS);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Self { root })
    }

    pub fn install(self, fdr: &[u8], image: &[u8], version: &[u8]) -> Result<()> {
        let apple = self
            .root
            .child("EFI", true, false)?
            .child("APPLE", true, false)?;
        install_generation(&apple, fdr, image, version)
    }
}

fn install_generation(apple: &Directory, fdr: &[u8], image: &[u8], version: &[u8]) -> Result<()> {
    if [fdr, image, version].iter().any(|b| b.is_empty()) {
        return Err(Error("incomplete EFI generation"));
    }
    if apple.exists("EMBEDDEDOS")? || apple.exists("EMBEDDEDOS.recovery")? {
        return Err(EXISTS);
    }
    let stage = apple.child("EMBEDDEDOS.recovery", true, false)?;
    for (name, bytes) in [
        ("FDRData", fdr),
        ("combined.memboot", image),
        ("version.plist", version),
    ] {
        save(&stage, name, bytes)?;
    }
    stage.sync()?;
    apple.sync()?;
    apple.rename("EMBEDDEDOS.recovery", apple, "EMBEDDEDOS")?;
    apple.sync().map_err(|_| Error("EFI generation was renamed but durability is uncertain; private recovery artifacts are retained"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_complete_and_preserved_data_is_never_replaced() {
        let path = std::env::temp_dir().join(format!(
            "t1-recovery-generation-synthetic-{}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        let apple = Directory::open(&path, false).unwrap();
        install_generation(
            &apple,
            b"synthetic fdr",
            b"synthetic image",
            b"synthetic version",
        )
        .unwrap();
        assert!(!apple.exists("EMBEDDEDOS.recovery").unwrap());
        assert_eq!(
            std::fs::read(path.join("EMBEDDEDOS/FDRData")).unwrap(),
            b"synthetic fdr"
        );
        assert!(install_generation(&apple, b"replacement", b"image", b"version").is_err());
        assert_eq!(
            std::fs::read(path.join("EMBEDDEDOS/FDRData")).unwrap(),
            b"synthetic fdr"
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn incomplete_stage_and_empty_artifacts_cannot_be_promoted() {
        let path = std::env::temp_dir().join(format!(
            "t1-recovery-interrupted-synthetic-{}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        let apple = Directory::open(&path, false).unwrap();
        assert!(install_generation(&apple, b"", b"synthetic", b"synthetic").is_err());
        assert!(!apple.exists("EMBEDDEDOS.recovery").unwrap());
        let stage = apple.child("EMBEDDEDOS.recovery", true, false).unwrap();
        save(&stage, "FDRData", b"synthetic partial").unwrap();
        assert!(install_generation(&apple, b"synthetic", b"synthetic", b"synthetic").is_err());
        assert!(!apple.exists("EMBEDDEDOS").unwrap());
        std::fs::remove_dir_all(path).unwrap();
    }
}
