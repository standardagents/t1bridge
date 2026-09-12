//! Behavioral acceptance for the external desktop-provider process boundary.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use t1_touchbar::desktop_provider::{
    DesktopAction, DesktopCapabilities, DesktopProvider, DesktopState, RendererFallback,
    notify_renderer_fallback,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_t1-desktop-provider-fixture");
static DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "t1bridge-desktop-provider-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn provider(&self, mode: &str) -> PathBuf {
        let path = self.0.join(mode);
        symlink(FIXTURE, &path).expect("link provider fixture");
        path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove test directory");
    }
}

fn wait_for_state(provider: &DesktopProvider, expected: DesktopState) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if provider.take_latest() == Some(expected) {
            return;
        }
        assert!(Instant::now() < deadline, "provider state deadline");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_log(path: &Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if fs::read_to_string(path).is_ok_and(|record| record == expected) {
            return;
        }
        assert!(Instant::now() < deadline, "provider action deadline");
        thread::sleep(Duration::from_millis(10));
    }
}

fn available_state() -> DesktopState {
    DesktopState {
        capabilities: DesktopCapabilities::AUDIO
            | DesktopCapabilities::MEDIA
            | DesktopCapabilities::NOTIFICATION,
        volume: Some(42),
        muted: false,
        display_off: false,
    }
}

#[test]
fn provider_status_and_actions_cross_the_fixed_process_contract() {
    let directory = TestDirectory::new();
    let path = directory.provider("valid-provider");
    let provider = DesktopProvider::start(&path).expect("start provider worker");
    wait_for_state(&provider, available_state());

    assert!(provider.dispatch(DesktopAction::SetVolume(73)));
    wait_for_log(&path.with_extension("log"), "v1 set-volume 73");
    assert!(provider.dispatch(DesktopAction::ToggleMute));
    wait_for_log(&path.with_extension("log"), "v1 toggle-mute");
    assert!(provider.dispatch(DesktopAction::MediaPrevious));
    wait_for_log(&path.with_extension("log"), "v1 media-previous");
    assert!(provider.dispatch(DesktopAction::MediaPlayPause));
    wait_for_log(&path.with_extension("log"), "v1 media-play-pause");
    assert!(provider.dispatch(DesktopAction::MediaNext));
    wait_for_log(&path.with_extension("log"), "v1 media-next");

    assert!(notify_renderer_fallback(
        &path,
        RendererFallback::SelectionExited
    ));
    wait_for_log(
        &path.with_extension("log"),
        "v1 notify-renderer-fallback selection-exited",
    );
}

#[test]
fn display_power_crosses_the_status_record_only_when_advertised() {
    let directory = TestDirectory::new();
    let path = directory.provider("display-off-provider");
    let provider = DesktopProvider::start(&path).expect("start provider worker");
    wait_for_state(
        &provider,
        DesktopState {
            capabilities: available_state().capabilities | DesktopCapabilities::DISPLAY_POWER,
            display_off: true,
            ..available_state()
        },
    );
}

#[test]
fn missing_notification_capability_uses_the_callers_journal_fallback() {
    let directory = TestDirectory::new();
    let path = directory.provider("no-notification-provider");
    assert!(!notify_renderer_fallback(
        &path,
        RendererFallback::SelectionUnavailable
    ));
    assert!(!path.with_extension("log").exists());
}

#[test]
fn malformed_oversized_failed_and_timed_out_providers_disable_their_state() {
    for mode in [
        "malformed-provider",
        "oversized-provider",
        "failed-provider",
        "timeout-provider",
    ] {
        let directory = TestDirectory::new();
        let path = directory.provider(mode);
        let provider = DesktopProvider::start(&path).expect("start failing provider worker");
        wait_for_state(&provider, DesktopState::default());
    }
}

#[test]
fn disappearing_provider_withdraws_only_its_advertised_state() {
    let directory = TestDirectory::new();
    let path = directory.provider("disappearing-provider");
    let provider = DesktopProvider::start(&path).expect("start provider worker");
    wait_for_state(&provider, available_state());

    fs::remove_file(&path).expect("remove exact provider fixture link");
    wait_for_state(&provider, DesktopState::default());
}
