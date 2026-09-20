use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;

use t1_daemons::auth_client::{TouchIdClientError, TouchIdCommand};
use t1_import::automatic::{AutomaticImportError, SourceError};
use t1_import::commit::CommitError;
use t1_import::fdr::MatchingRecordSelectionError;
use t1_import::runtime::{
    ProtectedImportError, attempt_protected_import, attempt_protected_import_from_backup,
};
use t1_import::status::{StatusError, inspect};
use t1_platform::diagnostics::{self, Component, Stage};
use t1_platform::preserved_efi_discovery;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ExitCategory {
    Usage = 2,
    Permission = 3,
    HardwareUnavailable = 20,
    AppleDataUnavailable = 21,
    AppleDataUnreadable = 22,
    AppleDataInvalid = 23,
    NoMatchingRecord = 24,
    ConflictingRecords = 25,
    AlreadyRunning = 26,
    StorageUnavailable = 27,
    StorageUnsafe = 28,
    DurabilityUncertain = 29,
    CommitFailed = 30,
    UsbCycleFailed = 31,
    RecoveryFailed = 32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Command {
    AutomaticImport,
    BackupImport(PathBuf),
    OnlineRecovery,
    Enroll,
    Match,
    Status,
    UsbCycle,
    UsbLiveLoss,
}

impl From<ExitCategory> for ExitCode {
    fn from(category: ExitCategory) -> Self {
        Self::from(category as u8)
    }
}

fn main() -> ExitCode {
    let mut arguments: Vec<_> = std::env::args_os().collect();
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--diagnostics")
    {
        diagnostics::enable();
        arguments.remove(1);
    }
    let Some(command) = parse_command(arguments) else {
        eprintln!(
            "usage: t1bridge [--diagnostics] machine-data import [--from ABSOLUTE_PATH] | machine-data recover --online | enroll | match | status | validate usb-cycle | validate usb-live-loss"
        );
        return ExitCategory::Usage.into();
    };
    let is_root = preserved_efi_discovery::is_root();
    if !authority_allows(&command, is_root) {
        diagnostics::native(
            Component::Importer,
            Stage::Authority,
            ExitCategory::Permission as i32,
        );
        eprintln!("t1bridge: {}", authority_error(&command));
        return ExitCategory::Permission.into();
    }
    match diagnostics::observe(Component::Importer, Stage::Startup, || run(command)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandError::Import(error)) => {
            eprintln!("t1bridge: {error}");
            let category = category_for_error(error);
            diagnostics::native(Component::Importer, Stage::Import, category as i32);
            category.into()
        }
        Err(CommandError::TouchId(error)) => {
            eprintln!("t1bridge: {error}");
            ExitCode::FAILURE
        }
        Err(CommandError::Status(error)) => {
            eprintln!("t1bridge: {error}");
            ExitCode::FAILURE
        }
        Err(CommandError::UsbCycle(error)) => {
            eprintln!("t1bridge: {error}");
            ExitCategory::UsbCycleFailed.into()
        }
        Err(CommandError::Recovery(error)) => {
            eprintln!("t1bridge: {error}");
            ExitCategory::RecoveryFailed.into()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandError {
    Import(ProtectedImportError),
    TouchId(TouchIdClientError),
    Status(StatusError),
    UsbCycle(t1_import::usb_cycle::Error),
    Recovery(t1_import::recovery::Error),
}

fn parse_command<I>(arguments: I) -> Option<Command>
where
    I: IntoIterator<Item = OsString>,
{
    let mut arguments = arguments.into_iter();
    let _program = arguments.next();
    match (arguments.next(), arguments.next(), arguments.next()) {
        (Some(group), Some(command), Some(option))
            if group == OsStr::new("machine-data")
                && command == OsStr::new("recover")
                && option == OsStr::new("--online")
                && arguments.next().is_none() =>
        {
            Some(Command::OnlineRecovery)
        }
        (Some(group), Some(command), Some(option))
            if group == OsStr::new("machine-data")
                && command == OsStr::new("import")
                && option == OsStr::new("--from") =>
        {
            let path = PathBuf::from(arguments.next()?);
            if !path.is_absolute() || arguments.next().is_some() {
                return None;
            }
            Some(Command::BackupImport(path))
        }
        (Some(group), Some(command), None)
            if group == OsStr::new("machine-data") && command == OsStr::new("import") =>
        {
            Some(Command::AutomaticImport)
        }
        (Some(command), None, None) if command == OsStr::new("enroll") => Some(Command::Enroll),
        (Some(command), None, None) if command == OsStr::new("match") => Some(Command::Match),
        (Some(command), None, None) if command == OsStr::new("status") => Some(Command::Status),
        (Some(group), Some(command), None)
            if group == OsStr::new("validate") && command == OsStr::new("usb-cycle") =>
        {
            Some(Command::UsbCycle)
        }
        (Some(group), Some(command), None)
            if group == OsStr::new("validate") && command == OsStr::new("usb-live-loss") =>
        {
            Some(Command::UsbLiveLoss)
        }
        _ => None,
    }
}

fn run(command: Command) -> Result<(), CommandError> {
    match command {
        Command::AutomaticImport => attempt_protected_import()
            .map(|_| ())
            .map_err(CommandError::Import),
        Command::BackupImport(path) => attempt_protected_import_from_backup(&path)
            .map(|_| ())
            .map_err(CommandError::Import),
        Command::OnlineRecovery => recover_and_import(t1_import::recovery::run, || {
            attempt_protected_import().map(|_| ())
        }),
        Command::Enroll => {
            t1_daemons::auth_client::run(TouchIdCommand::Enroll).map_err(CommandError::TouchId)
        }
        Command::Match => t1_daemons::auth_client::run(TouchIdCommand::Authenticate)
            .map_err(CommandError::TouchId),
        Command::Status => inspect()
            .map(|report| println!("{report}"))
            .map_err(CommandError::Status),
        Command::UsbCycle => t1_import::usb_cycle::run()
            .map(|report| println!("{report}"))
            .map_err(CommandError::UsbCycle),
        Command::UsbLiveLoss => t1_import::usb_cycle::run_live_loss()
            .map(|report| println!("{report}"))
            .map_err(CommandError::UsbCycle),
    }
}

fn recover_and_import(
    recover: impl FnOnce() -> Result<(), t1_import::recovery::Error>,
    import: impl FnOnce() -> Result<(), ProtectedImportError>,
) -> Result<(), CommandError> {
    recover().map_err(CommandError::Recovery)?;
    eprintln!("t1bridge: recovery tool completed; checking sensor-matched import");
    import().map_err(|error| {
        eprintln!("t1bridge: resolve the import error before retrying import; do not repeat firmware recovery");
        CommandError::Import(error)
    })?;
    println!(
        "t1bridge: recovered data accepted; next check status and fingerprint enrollment/matching"
    );
    Ok(())
}

const fn authority_allows(command: &Command, is_root: bool) -> bool {
    match command {
        Command::AutomaticImport
        | Command::BackupImport(_)
        | Command::OnlineRecovery
        | Command::Status
        | Command::UsbCycle
        | Command::UsbLiveLoss => is_root,
        Command::Enroll | Command::Match => !is_root,
    }
}

const fn authority_error(command: &Command) -> &'static str {
    match command {
        Command::AutomaticImport | Command::BackupImport(_) => {
            "machine-data import requires root authority"
        }
        Command::OnlineRecovery => "online recovery requires root authority",
        Command::Enroll => "enrollment requires a non-root user",
        Command::Match => "matching requires a non-root user",
        Command::Status => "status requires root authority",
        Command::UsbCycle | Command::UsbLiveLoss => "USB validation requires root authority",
    }
}

const fn category_for_error(error: ProtectedImportError) -> ExitCategory {
    match error {
        ProtectedImportError::StorageUnavailable => ExitCategory::StorageUnavailable,
        ProtectedImportError::Import(AutomaticImportError::Source(source)) => match source {
            SourceError::HardwareUnavailable => ExitCategory::HardwareUnavailable,
            SourceError::AppleDataUnavailable => ExitCategory::AppleDataUnavailable,
            SourceError::AppleDataUnreadable => ExitCategory::AppleDataUnreadable,
            SourceError::AppleDataInvalid => ExitCategory::AppleDataInvalid,
        },
        ProtectedImportError::Import(AutomaticImportError::Selection(selection)) => match selection
        {
            MatchingRecordSelectionError::NoMatchingRecord => ExitCategory::NoMatchingRecord,
            MatchingRecordSelectionError::ConflictingRecords { .. } => {
                ExitCategory::ConflictingRecords
            }
        },
        ProtectedImportError::Import(AutomaticImportError::Commit(commit)) => match commit {
            CommitError::AlreadyRunning => ExitCategory::AlreadyRunning,
            CommitError::InvalidDestination | CommitError::UnsafeOrphan => {
                ExitCategory::StorageUnsafe
            }
            CommitError::DurabilityUncertain => ExitCategory::DurabilityUncertain,
            CommitError::ReservationFailed
            | CommitError::DestinationInspectionFailed
            | CommitError::OrphanInspectionFailed
            | CommitError::OrphanRemovalFailed
            | CommitError::TemporaryCreationFailed
            | CommitError::TemporaryWriteFailed
            | CommitError::TemporarySyncFailed
            | CommitError::RenameFailed => ExitCategory::CommitFailed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_only_implemented_product_commands() {
        assert_eq!(
            parse_command(arguments(&["t1bridge", "machine-data", "import"])),
            Some(Command::AutomaticImport)
        );
        assert_eq!(
            parse_command(arguments(&["t1bridge", "enroll"])),
            Some(Command::Enroll)
        );
        assert_eq!(
            parse_command(arguments(&["t1bridge", "match"])),
            Some(Command::Match)
        );
        assert_eq!(
            parse_command(arguments(&["t1bridge", "status"])),
            Some(Command::Status)
        );
        assert_eq!(
            parse_command(arguments(&["t1bridge", "validate", "usb-cycle"])),
            Some(Command::UsbCycle)
        );
        assert_eq!(
            parse_command(arguments(&["t1bridge", "validate", "usb-live-loss"])),
            Some(Command::UsbLiveLoss)
        );
        for rejected in [
            arguments(&["t1bridge"]),
            arguments(&["t1bridge", "machine-data"]),
            arguments(&["t1bridge", "import"]),
            arguments(&["t1bridge", "validate"]),
            arguments(&["t1bridge", "validate", "usb-cycle", "synthetic-extra"]),
            arguments(&["t1bridge", "validate", "usb-live-loss", "synthetic-extra"]),
            arguments(&["t1bridge", "match", "synthetic-extra"]),
            arguments(&["t1bridge", "machine-data", "import", "synthetic-source"]),
            arguments(&["t1bridge", "machine-data", "import", "--association"]),
        ] {
            assert_eq!(parse_command(rejected), None);
        }
    }

    #[test]
    fn command_authority_matches_the_product_boundary() {
        for (command, root_allowed, user_allowed) in [
            (Command::AutomaticImport, true, false),
            (
                Command::BackupImport(PathBuf::from("/synthetic/backup")),
                true,
                false,
            ),
            (Command::Enroll, false, true),
            (Command::Match, false, true),
            (Command::Status, true, false),
            (Command::OnlineRecovery, true, false),
            (Command::UsbCycle, true, false),
            (Command::UsbLiveLoss, true, false),
        ] {
            assert_eq!(authority_allows(&command, true), root_allowed);
            assert_eq!(authority_allows(&command, false), user_allowed);
        }
    }

    #[test]
    fn backup_requires_one_explicit_absolute_source_without_identity_arguments() {
        assert_eq!(
            parse_command(arguments(&[
                "t1bridge",
                "machine-data",
                "import",
                "--from",
                "/synthetic/EFI backup",
            ])),
            Some(Command::BackupImport(PathBuf::from(
                "/synthetic/EFI backup"
            )))
        );
        for tail in [
            vec!["--from"],
            vec!["--from", ""],
            vec!["--from", "relative"],
            vec!["--from", "/synthetic", "extra"],
            vec!["--from", "/synthetic", "--association", "synthetic"],
        ] {
            let mut args = vec!["t1bridge", "machine-data", "import"];
            args.extend(tail);
            assert_eq!(parse_command(arguments(&args)), None);
        }
    }

    #[test]
    fn recovery_requires_online_opt_in_and_rejects_unattended_or_forced_flags() {
        assert_eq!(
            parse_command(arguments(&[
                "t1bridge",
                "machine-data",
                "recover",
                "--online"
            ])),
            Some(Command::OnlineRecovery)
        );
        for tail in [
            vec![],
            vec!["--no-confirm"],
            vec!["--online", "--no-confirm"],
            vec!["--online", "--force"],
            vec!["--online", "--from", "/synthetic/backup"],
            vec!["--online", "--tool", "/synthetic/tool"],
        ] {
            let mut args = vec!["t1bridge", "machine-data", "recover"];
            args.extend(tail);
            assert_eq!(parse_command(arguments(&args)), None);
        }
    }

    #[test]
    fn recovery_failure_never_reaches_import_or_retries() {
        use t1_import::recovery::Error;
        for error in [
            Error::Cancelled,
            Error::ToolUnavailable,
            Error::ToolFailed(Some(7)),
            Error::ToolFailed(None),
        ] {
            assert_eq!(
                recover_and_import(|| Err(error), || panic!("failed recovery must not import")),
                Err(CommandError::Recovery(error))
            );
        }
    }

    #[test]
    fn recovery_success_requires_a_successful_subsequent_import() {
        let recovered = std::cell::Cell::new(false);
        for import_result in [Ok(()), Err(ProtectedImportError::StorageUnavailable)] {
            recovered.set(false);
            assert_eq!(
                recover_and_import(
                    || {
                        recovered.set(true);
                        Ok(())
                    },
                    || {
                        assert!(recovered.get());
                        import_result
                    }
                ),
                import_result.map_err(CommandError::Import)
            );
        }
    }

    #[test]
    fn result_categories_are_stable_and_redaction_safe() {
        let cases = [
            (
                ProtectedImportError::StorageUnavailable,
                ExitCategory::StorageUnavailable,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Source(
                    SourceError::HardwareUnavailable,
                )),
                ExitCategory::HardwareUnavailable,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Source(
                    SourceError::AppleDataUnavailable,
                )),
                ExitCategory::AppleDataUnavailable,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Source(
                    SourceError::AppleDataUnreadable,
                )),
                ExitCategory::AppleDataUnreadable,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Source(
                    SourceError::AppleDataInvalid,
                )),
                ExitCategory::AppleDataInvalid,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Selection(
                    MatchingRecordSelectionError::NoMatchingRecord,
                )),
                ExitCategory::NoMatchingRecord,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Selection(
                    MatchingRecordSelectionError::ConflictingRecords { count: 2 },
                )),
                ExitCategory::ConflictingRecords,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Commit(
                    CommitError::AlreadyRunning,
                )),
                ExitCategory::AlreadyRunning,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Commit(
                    CommitError::UnsafeOrphan,
                )),
                ExitCategory::StorageUnsafe,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Commit(
                    CommitError::DurabilityUncertain,
                )),
                ExitCategory::DurabilityUncertain,
            ),
            (
                ProtectedImportError::Import(AutomaticImportError::Commit(
                    CommitError::TemporaryWriteFailed,
                )),
                ExitCategory::CommitFailed,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(category_for_error(error), expected);
            assert_ne!(expected as u8, u8::MAX);
        }
    }
}
