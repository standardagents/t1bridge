//! Fixed, bounded system-service control for the shared keybag relay.

use std::fmt;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::sep_lifecycle::KeybagRelayControl;

const SYSTEMCTL: &str = "/usr/bin/systemctl";
const RELAY_UNIT: &str = "t1bridge-keybag.service";
const WAIT_SLICE: Duration = Duration::from_millis(10);
const MAX_STATE_OUTPUT: u64 = 64;

const IS_ACTIVE_ARGS: &[&str] = &["is-active", RELAY_UNIT];
const START_ARGS: &[&str] = &["start", RELAY_UNIT];
const STOP_ARGS: &[&str] = &["stop", RELAY_UNIT];

/// Static, redacted keybag-relay control failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeybagRelayError {
    InvalidTimeout,
    ControlUnavailable,
    ControlWaitFailed,
    ControlTimedOut,
    ControlCleanupFailed,
    UnexpectedExit,
    UnknownState,
    VerificationFailed,
}

impl fmt::Display for KeybagRelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTimeout => "keybag relay control timeout is invalid",
            Self::ControlUnavailable => "keybag relay control is unavailable",
            Self::ControlWaitFailed => "keybag relay control wait failed",
            Self::ControlTimedOut => "keybag relay control timed out",
            Self::ControlCleanupFailed => "keybag relay control cleanup failed",
            Self::UnexpectedExit => "keybag relay control exited unexpectedly",
            Self::UnknownState => "keybag relay state is unknown",
            Self::VerificationFailed => "keybag relay state verification failed",
        })
    }
}

impl std::error::Error for KeybagRelayError {}

/// Production controller for the one fixed `T1Bridge` keybag service.
pub struct SystemctlKeybagRelay {
    control: RelayControl<ProcessCommandRunner>,
}

impl SystemctlKeybagRelay {
    /// Creates a controller with one caller-selected bound for each complete
    /// control operation, including post-action verification.
    ///
    /// # Errors
    ///
    /// Returns [`KeybagRelayError::InvalidTimeout`] for a zero duration.
    pub fn new(timeout: Duration) -> Result<Self, KeybagRelayError> {
        Ok(Self {
            control: RelayControl::new(ProcessCommandRunner, timeout)?,
        })
    }
}

impl KeybagRelayControl for SystemctlKeybagRelay {
    type Error = KeybagRelayError;

    fn is_active(&mut self) -> Result<bool, Self::Error> {
        self.control.is_active()
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.control.transition(ServiceAction::Stop)
    }

    fn start(&mut self) -> Result<(), Self::Error> {
        self.control.transition(ServiceAction::Start)
    }
}

struct RelayControl<Runner> {
    runner: Runner,
    timeout: Duration,
}

impl<Runner: CommandRunner> RelayControl<Runner> {
    fn new(runner: Runner, timeout: Duration) -> Result<Self, KeybagRelayError> {
        if timeout.is_zero() {
            return Err(KeybagRelayError::InvalidTimeout);
        }
        Ok(Self { runner, timeout })
    }

    fn is_active(&mut self) -> Result<bool, KeybagRelayError> {
        let deadline = self.deadline()?;
        self.query(deadline)
            .map(|state| state == ServiceState::Active)
    }

    fn transition(&mut self, action: ServiceAction) -> Result<(), KeybagRelayError> {
        let deadline = self.deadline()?;
        let action_result = self.runner.run(action.invocation(), deadline)?;
        let state = self.query(deadline)?;
        if !action_result.exit.success() {
            return Err(KeybagRelayError::UnexpectedExit);
        }
        if state != action.expected_state() {
            return Err(KeybagRelayError::VerificationFailed);
        }
        Ok(())
    }

    fn query(&mut self, deadline: Instant) -> Result<ServiceState, KeybagRelayError> {
        let result = self
            .runner
            .run(ServiceAction::Query.invocation(), deadline)?;
        match (result.exit, result.stdout.as_slice()) {
            (CommandExit::Code(0), b"active\n") => Ok(ServiceState::Active),
            (CommandExit::Code(3), b"inactive\n") => Ok(ServiceState::Inactive),
            (CommandExit::Code(3), b"failed\n") => Ok(ServiceState::Failed),
            _ => Err(KeybagRelayError::UnknownState),
        }
    }

    fn deadline(&self) -> Result<Instant, KeybagRelayError> {
        Instant::now()
            .checked_add(self.timeout)
            .ok_or(KeybagRelayError::InvalidTimeout)
    }
}

impl<Runner: CommandRunner> KeybagRelayControl for RelayControl<Runner> {
    type Error = KeybagRelayError;

    fn is_active(&mut self) -> Result<bool, Self::Error> {
        Self::is_active(self)
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.transition(ServiceAction::Stop)
    }

    fn start(&mut self) -> Result<(), Self::Error> {
        self.transition(ServiceAction::Start)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceState {
    Active,
    Inactive,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceAction {
    Query,
    Start,
    Stop,
}

impl ServiceAction {
    const fn invocation(self) -> CommandInvocation {
        let args = match self {
            Self::Query => IS_ACTIVE_ARGS,
            Self::Start => START_ARGS,
            Self::Stop => STOP_ARGS,
        };
        CommandInvocation {
            program: SYSTEMCTL,
            args,
            working_directory: "/",
            clear_environment: true,
            stdin: StdioPolicy::Null,
            stdout: match self {
                Self::Query => StdioPolicy::Capture,
                Self::Start | Self::Stop => StdioPolicy::Null,
            },
            stderr: StdioPolicy::Null,
        }
    }

    const fn expected_state(self) -> ServiceState {
        match self {
            Self::Start => ServiceState::Active,
            Self::Stop | Self::Query => ServiceState::Inactive,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StdioPolicy {
    Null,
    Capture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandInvocation {
    program: &'static str,
    args: &'static [&'static str],
    working_directory: &'static str,
    clear_environment: bool,
    stdin: StdioPolicy,
    stdout: StdioPolicy,
    stderr: StdioPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandExit {
    Code(i32),
    Signaled,
}

#[derive(Eq, PartialEq)]
struct CommandResult {
    exit: CommandExit,
    stdout: Vec<u8>,
}

impl fmt::Debug for CommandResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandResult")
            .field("exit", &self.exit)
            .field("stdout", &"[redacted]")
            .finish()
    }
}

impl CommandExit {
    const fn success(self) -> bool {
        matches!(self, Self::Code(0))
    }
}

trait CommandRunner {
    fn run(
        &mut self,
        invocation: CommandInvocation,
        deadline: Instant,
    ) -> Result<CommandResult, KeybagRelayError>;
}

struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(
        &mut self,
        invocation: CommandInvocation,
        deadline: Instant,
    ) -> Result<CommandResult, KeybagRelayError> {
        if Instant::now() >= deadline {
            return Err(KeybagRelayError::ControlTimedOut);
        }
        let mut command = Command::new(invocation.program);
        command.args(invocation.args);
        if invocation.clear_environment {
            command.env_clear();
        }
        command
            .current_dir(invocation.working_directory)
            .stdin(stdio(invocation.stdin))
            .stdout(stdio(invocation.stdout))
            .stderr(stdio(invocation.stderr));
        let mut child = command
            .spawn()
            .map_err(|_| KeybagRelayError::ControlUnavailable)?;
        let exit = wait_bounded(&mut child, deadline)?;
        let stdout = read_captured_stdout(&mut child, invocation.stdout)?;
        Ok(CommandResult { exit, stdout })
    }
}

fn stdio(policy: StdioPolicy) -> Stdio {
    match policy {
        StdioPolicy::Null => Stdio::null(),
        StdioPolicy::Capture => Stdio::piped(),
    }
}

fn read_captured_stdout(
    child: &mut Child,
    policy: StdioPolicy,
) -> Result<Vec<u8>, KeybagRelayError> {
    if policy == StdioPolicy::Null {
        return Ok(Vec::new());
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or(KeybagRelayError::ControlWaitFailed)?;
    let mut output = Vec::new();
    stdout
        .by_ref()
        .take(MAX_STATE_OUTPUT + 1)
        .read_to_end(&mut output)
        .map_err(|_| KeybagRelayError::ControlWaitFailed)?;
    if output.len() as u64 > MAX_STATE_OUTPUT {
        return Err(KeybagRelayError::UnknownState);
    }
    Ok(output)
}

fn wait_bounded(child: &mut Child, deadline: Instant) -> Result<CommandExit, KeybagRelayError> {
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| KeybagRelayError::ControlWaitFailed)?
        {
            return Ok(command_exit(status));
        }
        let now = Instant::now();
        if now >= deadline {
            terminate_and_reap(child)?;
            return Err(KeybagRelayError::ControlTimedOut);
        }
        thread::sleep(deadline.saturating_duration_since(now).min(WAIT_SLICE));
    }
}

fn terminate_and_reap(child: &mut Child) -> Result<(), KeybagRelayError> {
    if child.kill().is_err()
        && child
            .try_wait()
            .map_err(|_| KeybagRelayError::ControlCleanupFailed)?
            .is_none()
    {
        return Err(KeybagRelayError::ControlCleanupFailed);
    }
    child
        .wait()
        .map(|_| ())
        .map_err(|_| KeybagRelayError::ControlCleanupFailed)
}

fn command_exit(status: ExitStatus) -> CommandExit {
    status
        .code()
        .map_or(CommandExit::Signaled, CommandExit::Code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Clone, Copy)]
    struct ObservedCall {
        invocation: CommandInvocation,
        deadline: Instant,
    }

    struct FakeRunner {
        outcomes: VecDeque<Result<CommandResult, KeybagRelayError>>,
        calls: Vec<ObservedCall>,
    }

    impl FakeRunner {
        fn new(
            outcomes: impl IntoIterator<Item = Result<CommandResult, KeybagRelayError>>,
        ) -> Self {
            Self {
                outcomes: outcomes.into_iter().collect(),
                calls: Vec::new(),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &mut self,
            invocation: CommandInvocation,
            deadline: Instant,
        ) -> Result<CommandResult, KeybagRelayError> {
            self.calls.push(ObservedCall {
                invocation,
                deadline,
            });
            self.outcomes.pop_front().expect("one synthetic outcome")
        }
    }

    fn control(
        outcomes: impl IntoIterator<Item = Result<CommandResult, KeybagRelayError>>,
    ) -> RelayControl<FakeRunner> {
        RelayControl::new(FakeRunner::new(outcomes), Duration::from_secs(5)).unwrap()
    }

    fn result(exit: CommandExit, stdout: &[u8]) -> CommandResult {
        CommandResult {
            exit,
            stdout: stdout.to_vec(),
        }
    }

    fn action_ok() -> CommandResult {
        result(CommandExit::Code(0), b"")
    }

    fn active() -> CommandResult {
        result(CommandExit::Code(0), b"active\n")
    }

    fn inactive() -> CommandResult {
        result(CommandExit::Code(3), b"inactive\n")
    }

    #[test]
    fn exact_fixed_commands_clear_environment_and_capture_only_state() {
        let mut relay = control([
            Ok(active()),
            Ok(action_ok()),
            Ok(inactive()),
            Ok(action_ok()),
            Ok(active()),
        ]);
        assert!(relay.is_active().unwrap());
        relay.transition(ServiceAction::Stop).unwrap();
        relay.transition(ServiceAction::Start).unwrap();

        assert_eq!(
            relay
                .runner
                .calls
                .iter()
                .map(|call| call.invocation)
                .collect::<Vec<_>>(),
            [
                ServiceAction::Query.invocation(),
                ServiceAction::Stop.invocation(),
                ServiceAction::Query.invocation(),
                ServiceAction::Start.invocation(),
                ServiceAction::Query.invocation(),
            ]
        );
        for call in &relay.runner.calls {
            assert_eq!(call.invocation.program, SYSTEMCTL);
            assert_eq!(call.invocation.working_directory, "/");
            assert!(call.invocation.clear_environment);
            assert_eq!(call.invocation.stdin, StdioPolicy::Null);
            assert_eq!(call.invocation.stderr, StdioPolicy::Null);
        }
        assert_eq!(
            relay.runner.calls[0].invocation.stdout,
            StdioPolicy::Capture
        );
        assert_eq!(relay.runner.calls[1].invocation.stdout, StdioPolicy::Null);
        assert_eq!(
            relay.runner.calls[2].invocation.stdout,
            StdioPolicy::Capture
        );
        assert_eq!(relay.runner.calls[3].invocation.stdout, StdioPolicy::Null);
        assert_eq!(
            relay.runner.calls[4].invocation.stdout,
            StdioPolicy::Capture
        );
        assert_eq!(
            relay.runner.calls[1].deadline,
            relay.runner.calls[2].deadline
        );
        assert_eq!(
            relay.runner.calls[3].deadline,
            relay.runner.calls[4].deadline
        );
        assert_eq!(relay.runner.calls[0].invocation.args, IS_ACTIVE_ARGS);
        assert_eq!(relay.runner.calls[1].invocation.args, STOP_ARGS);
        assert_eq!(relay.runner.calls[2].invocation.args, IS_ACTIVE_ARGS);
        assert_eq!(relay.runner.calls[3].invocation.args, START_ARGS);
        assert_eq!(relay.runner.calls[4].invocation.args, IS_ACTIVE_ARGS);
    }

    #[test]
    fn health_accepts_only_exact_active_inactive_and_failed_results() {
        let mut active_control = control([Ok(active())]);
        assert_eq!(active_control.is_active(), Ok(true));
        let mut inactive_control = control([Ok(inactive())]);
        assert_eq!(inactive_control.is_active(), Ok(false));
        let mut failed_control = control([Ok(result(CommandExit::Code(3), b"failed\n"))]);
        assert_eq!(failed_control.is_active(), Ok(false));

        for outcome in [
            result(CommandExit::Code(0), b"failed\n"),
            result(CommandExit::Code(4), b"failed\n"),
            result(CommandExit::Code(3), b"failed"),
            result(CommandExit::Code(3), b"failed\nactive\n"),
            result(CommandExit::Code(3), b"active\n"),
            result(CommandExit::Code(3), b"activating\n"),
            result(CommandExit::Code(3), b"deactivating\n"),
            result(CommandExit::Code(4), b"unknown\n"),
            result(CommandExit::Code(0), b"inactive\n"),
            result(CommandExit::Signaled, b"active\n"),
        ] {
            let mut unknown = control([Ok(outcome)]);
            assert_eq!(unknown.is_active(), Err(KeybagRelayError::UnknownState));
        }
        let mut unavailable = control([Err(KeybagRelayError::ControlUnavailable)]);
        assert_eq!(
            unavailable.is_active(),
            Err(KeybagRelayError::ControlUnavailable)
        );
    }

    #[test]
    fn transitions_require_successful_exit_and_verified_result() {
        let mut start = control([Ok(action_ok()), Ok(active())]);
        start.transition(ServiceAction::Start).unwrap();
        let mut stop = control([Ok(action_ok()), Ok(inactive())]);
        stop.transition(ServiceAction::Stop).unwrap();

        let mut failed_start = control([Ok(result(CommandExit::Code(1), b"")), Ok(active())]);
        assert_eq!(
            failed_start.transition(ServiceAction::Start),
            Err(KeybagRelayError::UnexpectedExit)
        );
        assert_eq!(failed_start.runner.calls.len(), 2);

        let mut failed_stop = control([Ok(result(CommandExit::Code(1), b"")), Ok(inactive())]);
        assert_eq!(
            failed_stop.transition(ServiceAction::Stop),
            Err(KeybagRelayError::UnexpectedExit)
        );
        assert_eq!(failed_stop.runner.calls.len(), 2);
    }

    #[test]
    fn post_action_state_mismatch_is_a_typed_failure() {
        let mut start = control([Ok(action_ok()), Ok(inactive())]);
        assert_eq!(
            start.transition(ServiceAction::Start),
            Err(KeybagRelayError::VerificationFailed)
        );
        let mut stop = control([Ok(action_ok()), Ok(active())]);
        assert_eq!(
            stop.transition(ServiceAction::Stop),
            Err(KeybagRelayError::VerificationFailed)
        );
        // A stopped failure permits recovery, but is not a successful transition.
        for action in [ServiceAction::Start, ServiceAction::Stop] {
            let mut failed = control([
                Ok(action_ok()),
                Ok(result(CommandExit::Code(3), b"failed\n")),
            ]);
            assert_eq!(
                failed.transition(action),
                Err(KeybagRelayError::VerificationFailed)
            );
        }
    }

    #[cfg(feature = "auth-broker-service")]
    #[test]
    fn enrollment_recovers_a_failed_relay_only_after_bootstrap_and_verified_start() {
        use crate::live_standard_fingerprint::prepare_enrollment_relay;

        let mut relay = control([
            Ok(result(CommandExit::Code(3), b"failed\n")),
            Ok(action_ok()),
            Ok(active()),
            Ok(active()),
        ]);
        let mut bootstrapped = false;
        prepare_enrollment_relay(&mut relay, || {
            bootstrapped = true;
            Ok(())
        })
        .unwrap();
        assert!(bootstrapped);
        assert_eq!(
            relay
                .runner
                .calls
                .iter()
                .map(|call| call.invocation.args)
                .collect::<Vec<_>>(),
            [IS_ACTIVE_ARGS, START_ARGS, IS_ACTIVE_ARGS, IS_ACTIVE_ARGS]
        );
    }

    #[cfg(feature = "auth-broker-service")]
    #[test]
    fn enrollment_does_not_hide_bootstrap_restart_or_state_failures() {
        use crate::live_standard_fingerprint::{LiveStandardFailure, prepare_enrollment_relay};

        let mut relay = control([Ok(result(CommandExit::Code(3), b"failed\n"))]);
        assert_eq!(
            prepare_enrollment_relay(&mut relay, || Err(LiveStandardFailure::Cancelled)),
            Err(LiveStandardFailure::Cancelled)
        );
        assert_eq!(relay.runner.calls.len(), 1);

        for start in [action_ok(), result(CommandExit::Code(1), b"")] {
            let mut relay = control([
                Ok(result(CommandExit::Code(3), b"failed\n")),
                Ok(start),
                Ok(result(CommandExit::Code(3), b"failed\n")),
            ]);
            assert_eq!(
                prepare_enrollment_relay(&mut relay, || Ok(())),
                Err(LiveStandardFailure::Error)
            );
            assert_eq!(relay.runner.calls.len(), 3);
        }

        for state in [b"activating\n".as_slice(), b"deactivating\n", b"unknown\n"] {
            let mut relay = control([Ok(result(CommandExit::Code(3), state))]);
            assert_eq!(
                prepare_enrollment_relay(&mut relay, || panic!("must not bootstrap unknown state")),
                Err(LiveStandardFailure::Error)
            );
            assert_eq!(relay.runner.calls.len(), 1);
        }
        let mut relay = control([Ok(active())]);
        prepare_enrollment_relay(&mut relay, || panic!("active relay needs no bootstrap")).unwrap();
        assert_eq!(relay.runner.calls.len(), 1);
    }

    #[test]
    fn timeout_stops_the_transition_before_verification() {
        let mut relay = control([Err(KeybagRelayError::ControlTimedOut)]);
        assert_eq!(
            relay.transition(ServiceAction::Stop),
            Err(KeybagRelayError::ControlTimedOut)
        );
        assert_eq!(relay.runner.calls.len(), 1);
    }

    #[test]
    fn production_timeout_terminates_and_reaps_the_child() {
        let invocation = CommandInvocation {
            program: "/usr/bin/sleep",
            args: &["30"],
            working_directory: "/",
            clear_environment: true,
            stdin: StdioPolicy::Null,
            stdout: StdioPolicy::Null,
            stderr: StdioPolicy::Null,
        };
        let started = Instant::now();
        assert_eq!(
            ProcessCommandRunner.run(invocation, started + Duration::from_millis(20)),
            Err(KeybagRelayError::ControlTimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn command_result_debug_never_exposes_captured_output() {
        let formatted = format!(
            "{:?}",
            result(CommandExit::Code(4), b"machine-private-state")
        );
        assert_eq!(
            formatted,
            "CommandResult { exit: Code(4), stdout: \"[redacted]\" }"
        );
    }

    #[test]
    fn zero_timeout_is_rejected_before_any_control_command() {
        assert!(matches!(
            RelayControl::new(FakeRunner::new([]), Duration::ZERO),
            Err(KeybagRelayError::InvalidTimeout)
        ));
    }
}
