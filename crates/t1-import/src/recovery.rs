//! Explicit, attended handoff to the separately installed recovery tool.

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::{Command, ExitStatus};

const TOOL: &str = "/usr/bin/t1-revive";
const PROMPT: &str = "Experimental online recovery resets the T1, contacts Apple and writes EFI data.\n\
Use local EFI data or this Mac's backup first: t1bridge machine-data import [--from PATH].\n\
An import error does not establish data loss. Keep any surviving data backed up.\n\
Continue only after checking those sources and finding none usable.\n\
A failed recovery may require repair; stay at the terminal on mains power.\n\
Type recover to continue, or anything else to cancel: ";

/// Redacted failure from the optional recovery handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    TerminalRequired,
    ConfirmationFailed,
    Cancelled,
    ToolUnavailable,
    LaunchFailed,
    ToolFailed(Option<i32>),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TerminalRequired => formatter.write_str("online recovery requires an interactive terminal"),
            Self::ConfirmationFailed => formatter.write_str("could not confirm online recovery"),
            Self::Cancelled => formatter.write_str("online recovery cancelled; the recovery tool was not started"),
            Self::ToolUnavailable => formatter.write_str("install the optional t1-revive package first; see docs/online-recovery.md"),
            Self::LaunchFailed => formatter.write_str("could not start the installed t1-revive recovery tool"),
            Self::ToolFailed(Some(code)) => write!(formatter, "t1-revive stopped with exit {code}; inspect its recovery guidance before retrying; no import was attempted"),
            Self::ToolFailed(None) => formatter.write_str("t1-revive was interrupted; inspect its recovery guidance before retrying; no import was attempted"),
        }
    }
}

impl std::error::Error for Error {}

/// Starts recovery only after an explicit terminal acknowledgement.
///
/// The CLI checks root authority before calling this function. Local source
/// selection remains an operator decision: this command never infers that data
/// was lost from a hardware, mount, parser or storage error. No service or import
/// failure calls this entry point.
///
/// # Errors
/// Refuses unattended invocation, cancellation and unavailable tools. Returns
/// any unsuccessful tool termination without retrying recovery.
pub fn run() -> Result<(), Error> {
    let input = io::stdin();
    let output = io::stderr();
    let interactive = input.is_terminal() && output.is_terminal();
    // Release our input/output locks before the child inherits the terminal.
    confirm(&mut input.lock(), &mut output.lock(), interactive)?;
    launch(|| recovery_command().status())
}

fn recovery_command() -> Command {
    let mut command = Command::new(TOOL);
    command
        .args(["--confirm-each", "regenerate"])
        .current_dir("/")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        // Explicit presets override upstream's configuration defaults too.
        .env("T1R_NO_CONFIRM", "0")
        .env("T1R_DEMO", "0")
        .env("T1R_DRY_RUN", "0")
        .env("T1R_CONFIRM_EACH", "1");
    command
}

fn confirm(
    input: &mut impl BufRead,
    output: &mut impl Write,
    interactive: bool,
) -> Result<(), Error> {
    if !interactive {
        return Err(Error::TerminalRequired);
    }
    output
        .write_all(PROMPT.as_bytes())
        .and_then(|()| output.flush())
        .map_err(|_| Error::ConfirmationFailed)?;
    let mut answer = String::new();
    // Bound input and require a complete line; EOF cannot approve a restore.
    io::Read::take(input, 65)
        .read_line(&mut answer)
        .map_err(|_| Error::ConfirmationFailed)?;
    if answer != "recover\n" && answer != "recover\r\n" {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn launch(execute: impl FnOnce() -> io::Result<ExitStatus>) -> Result<(), Error> {
    let status = execute().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Error::ToolUnavailable
        } else {
            Error::LaunchFailed
        }
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::ToolFailed(status.code()))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io::Cursor;
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    #[test]
    fn requires_an_attended_explicit_complete_acknowledgement() {
        for answer in ["recover\n", "recover\r\n"] {
            assert_eq!(
                confirm(&mut Cursor::new(answer), &mut Vec::new(), true),
                Ok(())
            );
        }
        for answer in ["", "\n", "yes\n", "recover", "recover extra\n"] {
            assert_eq!(
                confirm(&mut Cursor::new(answer), &mut Vec::new(), true),
                Err(Error::Cancelled)
            );
        }
        let mut output = Vec::new();
        assert_eq!(
            confirm(&mut Cursor::new("recover\n"), &mut output, false),
            Err(Error::TerminalRequired)
        );
        assert!(output.is_empty());
    }

    #[test]
    fn confirmation_input_is_bounded_and_output_failure_cannot_approve() {
        let mut input = Cursor::new(vec![b'x'; 1_024]);
        assert_eq!(
            confirm(&mut input, &mut Vec::new(), true),
            Err(Error::Cancelled)
        );
        assert_eq!(input.position(), 65);
        let mut full_output = &mut [][..];
        assert_eq!(
            confirm(&mut Cursor::new("recover\n"), &mut full_output, true),
            Err(Error::ConfirmationFailed)
        );
    }

    #[test]
    fn every_failed_or_interrupted_recovery_stops_the_handoff() {
        assert_eq!(launch(|| Ok(ExitStatus::from_raw(0))), Ok(()));
        for code in 1..=7 {
            assert_eq!(
                launch(|| Ok(ExitStatus::from_raw(code << 8))),
                Err(Error::ToolFailed(Some(code)))
            );
        }
        assert_eq!(
            launch(|| Ok(ExitStatus::from_raw(15))),
            Err(Error::ToolFailed(None))
        );
        assert_eq!(
            launch(|| Err(io::ErrorKind::NotFound.into())),
            Err(Error::ToolUnavailable)
        );
        assert_eq!(
            launch(|| Err(io::ErrorKind::PermissionDenied.into())),
            Err(Error::LaunchFailed)
        );
    }

    #[test]
    fn handoff_uses_the_installed_tool_and_preserves_confirmation() {
        let command = recovery_command();
        assert_eq!(command.get_program(), TOOL);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--confirm-each", "regenerate"]
        );
        assert_eq!(command.get_current_dir(), Some(std::path::Path::new("/")));
        let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        for (key, value) in [
            ("T1R_NO_CONFIRM", "0"),
            ("T1R_DEMO", "0"),
            ("T1R_DRY_RUN", "0"),
            ("T1R_CONFIRM_EACH", "1"),
        ] {
            assert_eq!(
                environment.get(OsStr::new(key)),
                Some(&Some(OsStr::new(value)))
            );
        }
    }
}
