//! Concrete protected storage for the validated machine calibration record.

use std::fmt;
use std::fs::File;
use std::os::fd::AsFd;

use t1_platform::import_fs::{self, DirectoryComponent, Reservation};

use crate::commit::{DestinationState, ImportCommitStorage, OrphanState, StorageFailure};
use crate::fdr::MAX_FDR_RECORD_SIZE;

const STANDARD_DIRECTORY_MODE: u32 = 0o755;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;

const MACHINE_DATA_COMPONENTS: [DirectoryComponent<'static>; 4] = [
    DirectoryComponent::new("var", STANDARD_DIRECTORY_MODE),
    DirectoryComponent::new("lib", STANDARD_DIRECTORY_MODE),
    DirectoryComponent::new("t1bridge", PRIVATE_DIRECTORY_MODE),
    DirectoryComponent::new("machine-data", PRIVATE_DIRECTORY_MODE),
];

/// Production adapter for `/var/lib/t1bridge/machine-data/calibration.fscl`.
///
/// The fixed path has no machine identifier. Construction opens only the
/// trusted filesystem root; the native reservation later traverses each fixed
/// component without following links and requires root ownership.
pub struct MachineDataStorage {
    anchor: File,
    reservation: Option<Reservation>,
}

impl fmt::Debug for MachineDataStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MachineDataStorage([redacted])")
    }
}

impl MachineDataStorage {
    /// Opens the trusted filesystem root without creating or repairing state.
    ///
    /// # Errors
    ///
    /// Returns a redaction-safe failure if the root cannot be opened or
    /// inspected. Directory validation occurs when the commit is reserved.
    pub fn open() -> Result<Self, StorageFailure> {
        let anchor = File::open("/").map_err(|_| StorageFailure::Failed)?;
        Ok(Self {
            anchor,
            reservation: None,
        })
    }

    fn reservation(&mut self) -> Result<&mut Reservation, StorageFailure> {
        self.reservation.as_mut().ok_or(StorageFailure::Failed)
    }
}

impl ImportCommitStorage for MachineDataStorage {
    fn reserve_destination(&mut self, record_size: usize) -> Result<(), StorageFailure> {
        drop(self.reservation.take());
        // Automatic EFI discovery unshares the mount namespace after open().
        // A descriptor from the old namespace cannot traverse the cloned
        // ReadWritePaths mounts and reaches read-only storage instead. Acquire
        // the root again before pinning the destination for this transaction.
        self.anchor = File::open("/").map_err(|_| StorageFailure::Failed)?;
        self.reservation = Some(
            Reservation::reserve(
                self.anchor.as_fd(),
                &MACHINE_DATA_COMPONENTS,
                record_size,
                MAX_FDR_RECORD_SIZE,
            )
            .map_err(map_error)?,
        );
        Ok(())
    }

    fn inspect_destination(
        &mut self,
        expected_record: &[u8],
    ) -> Result<DestinationState, StorageFailure> {
        self.reservation()?
            .inspect_destination(expected_record)
            .map(|state| match state {
                import_fs::DestinationState::Absent => DestinationState::Absent,
                import_fs::DestinationState::Valid => DestinationState::Valid,
                import_fs::DestinationState::Invalid => DestinationState::Invalid,
            })
            .map_err(map_error)
    }

    fn inspect_orphan(&mut self) -> Result<OrphanState, StorageFailure> {
        self.reservation()?
            .inspect_orphan()
            .map(|state| match state {
                import_fs::OrphanState::Absent => OrphanState::Absent,
                import_fs::OrphanState::Validated => OrphanState::Validated,
                import_fs::OrphanState::Unsafe => OrphanState::Unsafe,
            })
            .map_err(map_error)
    }

    fn remove_validated_orphan(&mut self) -> Result<(), StorageFailure> {
        self.reservation()?
            .remove_validated_orphan()
            .map_err(map_error)
    }

    fn create_private_temporary(&mut self) -> Result<(), StorageFailure> {
        self.reservation()?
            .create_private_temporary()
            .map_err(map_error)
    }

    fn write_temporary(&mut self, record: &[u8]) -> Result<(), StorageFailure> {
        self.reservation()?
            .write_temporary(record)
            .map_err(map_error)
    }

    fn sync_temporary(&mut self) -> Result<(), StorageFailure> {
        self.reservation()?.sync_temporary().map_err(map_error)
    }

    fn rename_temporary(&mut self) -> Result<(), StorageFailure> {
        self.reservation()?.rename_temporary().map_err(map_error)
    }

    fn sync_destination_directory(&mut self) -> Result<(), StorageFailure> {
        self.reservation()?
            .sync_destination_directory()
            .map_err(map_error)
    }
}

fn map_error(error: import_fs::Error) -> StorageFailure {
    if error == import_fs::Error::AlreadyRunning {
        StorageFailure::AlreadyRunning
    } else {
        StorageFailure::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commit::{CommitOutcome, commit_fdr_calibration};
    use crate::fdr::FdrCalibrationRecord;
    use std::fs;
    use std::process::Command;

    #[test]
    fn commit_uses_current_mount_namespace() {
        const RECORD: &[u8] = b"SYNTHETIC-CALIBRATION-RECORD";
        if let Ok(descriptor) = std::env::var("T1BRIDGE_TEST_STALE_ROOT") {
            let descriptor: u32 = descriptor.parse().unwrap();
            let anchor = File::open(format!("/proc/self/fd/{descriptor}")).unwrap();
            // This is the anchor opened before discovery changed namespaces.
            // Prove the fixture reproduces the service's read-only traversal.
            let mut stale = Reservation::reserve(
                anchor.as_fd(),
                &MACHINE_DATA_COMPONENTS,
                RECORD.len(),
                MAX_FDR_RECORD_SIZE,
            )
            .unwrap();
            assert_eq!(
                stale.create_private_temporary(),
                Err(import_fs::Error::Failed)
            );
            drop(stale);

            let mut storage = MachineDataStorage {
                anchor,
                reservation: None,
            };
            for expected in [CommitOutcome::Installed, CommitOutcome::AlreadyInstalled] {
                assert_eq!(
                    commit_fdr_calibration(
                        &mut storage,
                        FdrCalibrationRecord::from_validated_test_bytes(RECORD),
                    ),
                    Ok(expected),
                );
            }
            assert_eq!(
                fs::read("/var/lib/t1bridge/machine-data/calibration.fscl").unwrap(),
                RECORD,
            );
            assert!(
                !std::path::Path::new("/var/lib/t1bridge/machine-data/.calibration.fscl.tmp")
                    .exists()
            );
            return;
        }

        let available = Command::new("unshare")
            .args(["--user", "--map-root-user", "--mount", "true"])
            .output()
            .is_ok_and(|output| output.status.success());
        if !available {
            eprintln!("skipping import namespace regression: user/mount namespaces unavailable");
            assert!(std::env::var_os("T1BRIDGE_REQUIRE_NAMESPACE_TEST").is_none());
            return;
        }

        let directory =
            std::env::temp_dir().join(format!("t1-import-namespace-test-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let fixture = directory.join("fixture");
        let root = directory.join("root");
        fs::create_dir(&root).unwrap();
        let build = Command::new("cc")
            .args(["-std=c17", "-Wall", "-Wextra", "-Werror"])
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/c/test_import_namespace.c"
            ))
            .arg("-o")
            .arg(&fixture)
            .output()
            .unwrap();
        assert!(build.status.success(), "{build:?}");
        let result = Command::new("unshare")
            .args(["--user", "--map-root-user", "--mount"])
            .arg(&fixture)
            .arg(std::env::current_exe().unwrap())
            .arg(&root)
            .output()
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
        assert!(result.status.success(), "{result:?}");
    }

    #[test]
    fn preserves_the_nonblocking_reservation_result() {
        assert_eq!(
            map_error(import_fs::Error::AlreadyRunning),
            StorageFailure::AlreadyRunning
        );
        assert_eq!(map_error(import_fs::Error::Failed), StorageFailure::Failed);
    }
}
