//! Optional, distribution-owned desktop integration for the user renderer.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    Arc, Condvar, Mutex,
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;
use std::time::{Duration, Instant};

pub const PROVIDER_ENVIRONMENT: &str = "T1BRIDGE_DESKTOP_PROVIDER";

const STATUS_MAGIC: &str = "T1BRIDGE-DESKTOP";
const STATUS_VERSION: &str = "1";
const STATUS_OUTPUT_LIMIT: usize = 128;
const CALL_DEADLINE: Duration = Duration::from_millis(500);
const STATUS_INTERVAL: Duration = Duration::from_secs(1);
const WAIT_INTERVAL: Duration = Duration::from_millis(5);
const ACTION_QUEUE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DesktopCapabilities(u8);

impl DesktopCapabilities {
    pub const AUDIO: Self = Self(1);
    pub const MEDIA: Self = Self(2);
    pub const NOTIFICATION: Self = Self(4);
    pub const LEVEL_FEEDBACK: Self = Self(8);
    pub const DISPLAY_POWER: Self = Self(16);
    const ALL: u8 = Self::AUDIO.0
        | Self::MEDIA.0
        | Self::NOTIFICATION.0
        | Self::LEVEL_FEEDBACK.0
        | Self::DISPLAY_POWER.0;

    #[must_use]
    pub const fn contains(self, capability: Self) -> bool {
        self.0 & capability.0 == capability.0
    }
}

impl std::ops::BitOr for DesktopCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DesktopState {
    pub capabilities: DesktopCapabilities,
    pub volume: Option<u8>,
    pub muted: bool,
    /// True only while a provider advertising display power reports the
    /// desktop display off. An absent or failed provider never blanks.
    pub display_off: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopAction {
    SetVolume(u8),
    ToggleMute,
    MediaPrevious,
    MediaPlayPause,
    MediaNext,
    ShowDisplayBrightness,
    ShowKeyboardBacklight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererFallback {
    SelectionUnavailable,
    SelectionExited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopProviderError {
    Spawn,
    Timeout,
    Failed,
    Output,
    Status,
}

impl fmt::Display for DesktopProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Spawn => "desktop provider could not start",
            Self::Timeout => "desktop provider timed out",
            Self::Failed => "desktop provider rejected the operation",
            Self::Output => "desktop provider output failed",
            Self::Status => "desktop provider status is invalid",
        })
    }
}

impl std::error::Error for DesktopProviderError {}

/// Resolves one already-read provider environment value.
///
/// Only an absolute executable regular file is accepted. Symlinks are followed
/// by normal filesystem metadata resolution.
#[must_use]
pub fn resolve_provider_path(value: Option<&OsStr>) -> Option<PathBuf> {
    let path = PathBuf::from(value.filter(|value| !value.is_empty())?);
    if !path.is_absolute() {
        return None;
    }
    let metadata = fs::metadata(&path).ok()?;
    (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(path)
}

/// Sends the optional provider notification synchronously at renderer fallback.
///
/// Failure is reported to the caller so it can emit the one journal diagnostic.
#[must_use]
pub fn notify_renderer_fallback(path: &Path, reason: RendererFallback) -> bool {
    let Ok(state) = query_status(path) else {
        return false;
    };
    if !state
        .capabilities
        .contains(DesktopCapabilities::NOTIFICATION)
    {
        return false;
    }
    run_action(path, ProviderOperation::NotifyRendererFallback(reason)).is_ok()
}

/// Nonblocking handle to one bounded provider worker.
pub struct DesktopProvider {
    actions: Arc<ActionMailbox>,
    states: Receiver<DesktopState>,
}

#[derive(Debug, Default)]
struct ActionQueue {
    pending: VecDeque<DesktopAction>,
    closed: bool,
}

#[derive(Debug, Default)]
struct ActionMailbox {
    queue: Mutex<ActionQueue>,
    ready: Condvar,
}

impl DesktopProvider {
    /// Starts a worker only for a currently valid provider path.
    #[must_use]
    pub fn start(path: &Path) -> Option<Self> {
        let path = resolve_provider_path(Some(path.as_os_str()))?;
        let (state_sender, state_receiver) = mpsc::channel();
        let actions = Arc::new(ActionMailbox::default());
        let worker_actions = Arc::clone(&actions);
        thread::Builder::new()
            .name("t1-desktop-provider".into())
            .spawn(move || provider_worker(&path, &worker_actions, &state_sender))
            .ok()?;
        Some(Self {
            actions,
            states: state_receiver,
        })
    }

    /// Returns the newest available status and discards superseded snapshots.
    #[must_use]
    pub fn take_latest(&self) -> Option<DesktopState> {
        let mut latest = None;
        loop {
            match self.states.try_recv() {
                Ok(state) => latest = Some(state),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return latest,
            }
        }
    }

    /// Queues one action without blocking display or input work.
    ///
    /// Consecutive volume positions replace the pending position instead of
    /// building a drag backlog. Returns false when the bounded action queue is
    /// full or the worker is gone.
    #[must_use]
    pub fn dispatch(&self, action: DesktopAction) -> bool {
        let Ok(mut queue) = self.actions.queue.lock() else {
            return false;
        };
        if queue.closed {
            return false;
        }
        let action = match action {
            DesktopAction::SetVolume(value) => DesktopAction::SetVolume(value.min(100)),
            action => action,
        };
        if matches!(action, DesktopAction::SetVolume(_))
            && let Some(pending @ DesktopAction::SetVolume(_)) = queue.pending.back_mut()
        {
            *pending = action;
        } else if queue.pending.len() < ACTION_QUEUE_CAPACITY {
            queue.pending.push_back(action);
        } else {
            return false;
        }
        self.actions.ready.notify_one();
        true
    }
}

impl Drop for DesktopProvider {
    fn drop(&mut self) {
        if let Ok(mut queue) = self.actions.queue.lock() {
            queue.closed = true;
            self.actions.ready.notify_one();
        }
    }
}

fn provider_worker(path: &Path, actions: &ActionMailbox, states: &mpsc::Sender<DesktopState>) {
    let mut available = None;
    loop {
        let state = if let Ok(state) = query_status(path) {
            report_availability(&mut available, true);
            state
        } else {
            report_availability(&mut available, false);
            DesktopState::default()
        };
        // Publish every authoritative poll, including an unchanged value. The
        // renderer may be displaying an optimistic action result that a failed
        // or no-op provider did not actually apply.
        if states.send(state).is_err() {
            return;
        }

        let refresh_at = Instant::now() + STATUS_INTERVAL;
        loop {
            let Some(action) = take_action(actions, refresh_at) else {
                if actions.queue.lock().is_ok_and(|queue| queue.closed) {
                    return;
                }
                break;
            };
            if run_action(path, ProviderOperation::Desktop(action)).is_err() {
                report_availability(&mut available, false);
            }
        }
    }
}

fn take_action(actions: &ActionMailbox, deadline: Instant) -> Option<DesktopAction> {
    let mut queue = actions.queue.lock().ok()?;
    loop {
        if queue.closed {
            return None;
        }
        if let Some(action) = queue.pending.pop_front() {
            return Some(action);
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        if wait.is_zero() {
            return None;
        }
        let (next, timed) = actions.ready.wait_timeout(queue, wait).ok()?;
        queue = next;
        if timed.timed_out() {
            return None;
        }
    }
}

fn report_availability(previous: &mut Option<bool>, current: bool) {
    use t1_platform::diagnostics::{Component, Outcome, Record, Stage, emit};
    if *previous == Some(current) {
        return;
    }
    emit(Record::new(
        Component::Provider,
        Stage::ProviderStatus,
        if current { Outcome::Ok } else { Outcome::Error },
        None,
    ));
    if !current {
        eprintln!("t1-touchbar: configured desktop provider is unavailable");
    }
    *previous = Some(current);
}

fn query_status(path: &Path) -> Result<DesktopState, DesktopProviderError> {
    let output = run_provider(path, &["v1", "status"], true)?;
    parse_status(&output)
}

#[derive(Clone, Copy)]
enum ProviderOperation {
    Desktop(DesktopAction),
    NotifyRendererFallback(RendererFallback),
}

fn run_action(path: &Path, operation: ProviderOperation) -> Result<(), DesktopProviderError> {
    let arguments = action_arguments(operation);
    let borrowed: Vec<_> = arguments.iter().map(OsString::as_os_str).collect();
    t1_platform::diagnostics::observe(
        t1_platform::diagnostics::Component::Provider,
        t1_platform::diagnostics::Stage::ProviderAction,
        || run_provider(path, &borrowed, false),
    )
    .map(|_| ())
}

fn action_arguments(operation: ProviderOperation) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("v1")];
    match operation {
        ProviderOperation::Desktop(DesktopAction::SetVolume(value)) => {
            arguments.push(OsString::from("set-volume"));
            arguments.push(OsString::from(value.min(100).to_string()));
        }
        ProviderOperation::Desktop(DesktopAction::ToggleMute) => {
            arguments.push(OsString::from("toggle-mute"));
        }
        ProviderOperation::Desktop(DesktopAction::MediaPrevious) => {
            arguments.push(OsString::from("media-previous"));
        }
        ProviderOperation::Desktop(DesktopAction::MediaPlayPause) => {
            arguments.push(OsString::from("media-play-pause"));
        }
        ProviderOperation::Desktop(DesktopAction::MediaNext) => {
            arguments.push(OsString::from("media-next"));
        }
        ProviderOperation::Desktop(DesktopAction::ShowDisplayBrightness) => {
            arguments.push(OsString::from("show-display-brightness"));
        }
        ProviderOperation::Desktop(DesktopAction::ShowKeyboardBacklight) => {
            arguments.push(OsString::from("show-keyboard-backlight"));
        }
        ProviderOperation::NotifyRendererFallback(reason) => {
            arguments.push(OsString::from("notify-renderer-fallback"));
            arguments.push(OsString::from(match reason {
                RendererFallback::SelectionUnavailable => "selection-unavailable",
                RendererFallback::SelectionExited => "selection-exited",
            }));
        }
    }
    arguments
}

fn run_provider(
    path: &Path,
    arguments: &[impl AsRef<OsStr>],
    capture_output: bool,
) -> Result<Vec<u8>, DesktopProviderError> {
    let mut command = Command::new(path);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let mut reader = if capture_output {
        let (reader, writer) = UnixStream::pair().map_err(|_| DesktopProviderError::Output)?;
        reader
            .set_nonblocking(true)
            .map_err(|_| DesktopProviderError::Output)?;
        let descriptor: OwnedFd = writer.into();
        command.stdout(Stdio::from(descriptor));
        Some(reader)
    } else {
        command.stdout(Stdio::null());
        None
    };
    let mut child = command.spawn().map_err(|_| DesktopProviderError::Spawn)?;
    let deadline = Instant::now() + CALL_DEADLINE;
    let mut output = Vec::new();
    loop {
        if let Some(reader) = reader.as_mut()
            && let Err(error) = read_available(reader, &mut output)
        {
            terminate(&mut child);
            return Err(error);
        }
        match child.try_wait().map_err(|_| DesktopProviderError::Failed)? {
            Some(status) => {
                if let Some(reader) = reader.as_mut()
                    && let Err(error) = read_available(reader, &mut output)
                {
                    terminate(&mut child);
                    return Err(error);
                }
                return finish_status(status, output);
            }
            None if Instant::now() >= deadline => {
                terminate(&mut child);
                return Err(DesktopProviderError::Timeout);
            }
            None => thread::sleep(WAIT_INTERVAL),
        }
    }
}

fn read_available(
    reader: &mut UnixStream,
    output: &mut Vec<u8>,
) -> Result<(), DesktopProviderError> {
    let mut buffer = [0_u8; 64];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(length) => {
                if output.len().saturating_add(length) > STATUS_OUTPUT_LIMIT {
                    return Err(DesktopProviderError::Output);
                }
                output.extend_from_slice(&buffer[..length]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(DesktopProviderError::Output),
        }
    }
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn finish_status(status: ExitStatus, output: Vec<u8>) -> Result<Vec<u8>, DesktopProviderError> {
    status
        .success()
        .then_some(output)
        .ok_or(DesktopProviderError::Failed)
}

fn parse_status(output: &[u8]) -> Result<DesktopState, DesktopProviderError> {
    let text = std::str::from_utf8(output).map_err(|_| DesktopProviderError::Status)?;
    let fields: Vec<_> = text.split_ascii_whitespace().collect();
    let (magic, version, capability_field, volume_field, muted_field, display_field) =
        match fields.as_slice() {
            [magic, version, capabilities, volume, muted] => {
                (*magic, *version, *capabilities, *volume, *muted, None)
            }
            [magic, version, capabilities, volume, muted, display] => (
                *magic,
                *version,
                *capabilities,
                *volume,
                *muted,
                Some(*display),
            ),
            _ => return Err(DesktopProviderError::Status),
        };
    if magic != STATUS_MAGIC || version != STATUS_VERSION {
        return Err(DesktopProviderError::Status);
    }
    let bits = capability_field
        .parse::<u8>()
        .map_err(|_| DesktopProviderError::Status)?;
    if bits & !DesktopCapabilities::ALL != 0 {
        return Err(DesktopProviderError::Status);
    }
    let capabilities = DesktopCapabilities(bits);
    let (volume, muted) = if capabilities.contains(DesktopCapabilities::AUDIO) {
        let volume = volume_field
            .parse::<u8>()
            .map_err(|_| DesktopProviderError::Status)?;
        if volume > 100 {
            return Err(DesktopProviderError::Status);
        }
        let muted = parse_flag(muted_field)?;
        (Some(volume), muted)
    } else if volume_field == "-" && muted_field == "-" {
        (None, false)
    } else {
        return Err(DesktopProviderError::Status);
    };
    // The display field exists exactly when display power is advertised, so
    // a minor-zero renderer keeps rejecting the bit and a minor-zero provider
    // keeps sending five fields.
    let display_off = match (
        capabilities.contains(DesktopCapabilities::DISPLAY_POWER),
        display_field,
    ) {
        (true, Some(display)) => !parse_flag(display)?,
        (false, None) => false,
        _ => return Err(DesktopProviderError::Status),
    };
    Ok(DesktopState {
        capabilities,
        volume,
        muted,
        display_off,
    })
}

fn parse_flag(field: &str) -> Result<bool, DesktopProviderError> {
    match field {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(DesktopProviderError::Status),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn status_parser_accepts_exact_capability_state_combinations() {
        assert_eq!(
            parse_status(b"T1BRIDGE-DESKTOP 1 15 42 1\n"),
            Ok(DesktopState {
                capabilities: DesktopCapabilities::AUDIO
                    | DesktopCapabilities::MEDIA
                    | DesktopCapabilities::NOTIFICATION
                    | DesktopCapabilities::LEVEL_FEEDBACK,
                volume: Some(42),
                muted: true,
                display_off: false,
            })
        );
        assert_eq!(
            parse_status(b"T1BRIDGE-DESKTOP 1 6 - -\n"),
            Ok(DesktopState {
                capabilities: DesktopCapabilities::MEDIA | DesktopCapabilities::NOTIFICATION,
                volume: None,
                muted: false,
                display_off: false,
            })
        );
    }

    #[test]
    fn status_parser_reads_display_power_only_when_advertised() {
        assert_eq!(
            parse_status(b"T1BRIDGE-DESKTOP 1 17 42 0 0\n"),
            Ok(DesktopState {
                capabilities: DesktopCapabilities::AUDIO | DesktopCapabilities::DISPLAY_POWER,
                volume: Some(42),
                muted: false,
                display_off: true,
            })
        );
        assert_eq!(
            parse_status(b"T1BRIDGE-DESKTOP 1 16 - - 1\n"),
            Ok(DesktopState {
                capabilities: DesktopCapabilities::DISPLAY_POWER,
                volume: None,
                muted: false,
                display_off: false,
            })
        );
        for invalid in [
            b"T1BRIDGE-DESKTOP 1 16 - -".as_slice(),
            b"T1BRIDGE-DESKTOP 1 16 - - 2",
            b"T1BRIDGE-DESKTOP 1 16 - - -",
            b"T1BRIDGE-DESKTOP 1 1 42 0 1",
            b"T1BRIDGE-DESKTOP 1 17 42 0",
        ] {
            assert_eq!(parse_status(invalid), Err(DesktopProviderError::Status));
        }
    }

    #[test]
    fn status_parser_rejects_ambiguous_or_unbounded_state() {
        for invalid in [
            b"T1BRIDGE-DESKTOP 2 7 42 0".as_slice(),
            b"T1BRIDGE-DESKTOP 1 32 - -",
            b"T1BRIDGE-DESKTOP 1 1 - -",
            b"T1BRIDGE-DESKTOP 1 0 42 0",
            b"T1BRIDGE-DESKTOP 1 1 101 0",
            b"T1BRIDGE-DESKTOP 1 1 42 2",
            b"T1BRIDGE-DESKTOP 1 0 - - trailing",
        ] {
            assert_eq!(parse_status(invalid), Err(DesktopProviderError::Status));
        }
    }

    #[test]
    fn action_arguments_are_fixed_and_values_are_bounded() {
        assert_eq!(
            action_arguments(ProviderOperation::Desktop(DesktopAction::SetVolume(255))),
            ["v1", "set-volume", "100"]
        );
        assert_eq!(
            action_arguments(ProviderOperation::Desktop(DesktopAction::MediaPlayPause)),
            ["v1", "media-play-pause"]
        );
        assert_eq!(
            action_arguments(ProviderOperation::Desktop(
                DesktopAction::ShowDisplayBrightness
            )),
            ["v1", "show-display-brightness"]
        );
        assert_eq!(
            action_arguments(ProviderOperation::Desktop(
                DesktopAction::ShowKeyboardBacklight
            )),
            ["v1", "show-keyboard-backlight"]
        );
        assert_eq!(
            action_arguments(ProviderOperation::NotifyRendererFallback(
                RendererFallback::SelectionExited,
            )),
            ["v1", "notify-renderer-fallback", "selection-exited"]
        );
    }

    #[test]
    fn volume_drag_retains_only_the_latest_pending_position() {
        let (_state_sender, state_receiver) = mpsc::channel();
        let provider = DesktopProvider {
            actions: Arc::new(ActionMailbox::default()),
            states: state_receiver,
        };

        assert!(provider.dispatch(DesktopAction::SetVolume(20)));
        assert!(provider.dispatch(DesktopAction::SetVolume(40)));
        assert!(provider.dispatch(DesktopAction::SetVolume(80)));
        assert_eq!(
            take_action(
                &provider.actions,
                Instant::now() + Duration::from_millis(10)
            ),
            Some(DesktopAction::SetVolume(80))
        );
    }

    #[test]
    fn volume_coalescing_preserves_discrete_action_order() {
        let (_state_sender, state_receiver) = mpsc::channel();
        let provider = DesktopProvider {
            actions: Arc::new(ActionMailbox::default()),
            states: state_receiver,
        };

        assert!(provider.dispatch(DesktopAction::SetVolume(20)));
        assert!(provider.dispatch(DesktopAction::ToggleMute));
        assert!(provider.dispatch(DesktopAction::SetVolume(40)));
        assert!(provider.dispatch(DesktopAction::SetVolume(80)));
        for expected in [
            DesktopAction::SetVolume(20),
            DesktopAction::ToggleMute,
            DesktopAction::SetVolume(80),
        ] {
            assert_eq!(
                take_action(
                    &provider.actions,
                    Instant::now() + Duration::from_millis(10)
                ),
                Some(expected)
            );
        }
    }

    #[test]
    fn provider_path_requires_an_absolute_executable_file() {
        let executable = std::env::current_exe().expect("current test executable");
        assert_eq!(
            resolve_provider_path(Some(executable.as_os_str())),
            Some(executable)
        );
        assert_eq!(
            resolve_provider_path(Some(OsStr::new("relative/provider"))),
            None
        );
        assert_eq!(resolve_provider_path(None), None);

        let directory = std::env::temp_dir();
        assert_eq!(resolve_provider_path(Some(directory.as_os_str())), None);
        let mode = fs::metadata(std::env::current_exe().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
    }
}
