//! Allowlisted diagnostics shared by the userspace services. Never format inputs.

use std::fmt;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static ENV_ENABLED: OnceLock<bool> = OnceLock::new();
static RECORDS: AtomicUsize = AtomicUsize::new(0);
const RECORD_LIMIT: usize = 4096;

macro_rules! labels {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name { $($variant),+ }
        impl $name {
            /// Returns the exact wire-format label this variant emits.
            pub const fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}

labels!(Component {
    Importer => "importer", Efi => "efi", Sep => "sep", Broker => "broker",
    Ncm => "ncm", Xart => "xart", TouchbarHardware => "touchbar-hardware",
    Renderer => "renderer", Provider => "provider",
});

labels!(Stage {
    Startup => "startup", Enroll => "enroll", Relay => "relay",
    Preparation => "preparation", Transaction => "transaction",
    Import => "import", Authority => "authority", StorageOpen => "storage-open",
    HardwareAssociation => "hardware-association", EfiDiscovery => "efi-discovery",
    EfiOpen => "efi-open", EfiRead => "efi-read", EfiParse => "efi-parse",
    Selection => "selection", Commit => "commit", NcmPrepare => "ncm-prepare",
    XartBind => "xart-bind", XartAccept => "xart-accept", XartSession => "xart-session",
    DisplayOpen => "display-open", DigitizerOpen => "digitizer-open", FnOpen => "fn-open",
    KeyboardCreate => "keyboard-create", SessionWatch => "session-watch",
    SessionAdmission => "session-admission", SessionRevocation => "session-revocation",
    Frame => "frame", RendererConnect => "renderer-connect",
    RendererProtocol => "renderer-protocol", ProviderAction => "provider-action",
    ProviderStatus => "provider-status", Match => "match", Delete => "delete",
    List => "list", SepLease => "sep-lease", SepCleanup => "sep-cleanup", Limit => "limit",
    KeystoreReply => "keystore-reply", KeystoreOuter => "keystore-outer",
    KeystoreInner => "keystore-inner",
    Overlay => "overlay", OverlayPress => "overlay-press", TouchIdCancel => "touchid-cancel",
});

labels!(Outcome {
    Begin => "begin", Ok => "ok", Error => "error", Limit => "limit",
});

/// Enables diagnostics explicitly before starting workers; environment opt-in is
/// also supported with exactly `T1BRIDGE_DIAGNOSTICS=1`.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
        || *ENV_ENABLED
            .get_or_init(|| requested(std::env::var_os("T1BRIDGE_DIAGNOSTICS").as_deref()))
}

fn requested(value: Option<&std::ffi::OsStr>) -> bool {
    value == Some(std::ffi::OsStr::new("1"))
}

/// Contains only closed labels and numeric protocol status, never caller data.
#[derive(Clone, Copy)]
pub struct Record {
    component: Component,
    stage: Stage,
    outcome: Outcome,
    code: Option<i64>,
    command: Option<u16>,
    selector: Option<u8>,
}

impl Record {
    #[must_use]
    pub const fn new(
        component: Component,
        stage: Stage,
        outcome: Outcome,
        code: Option<i64>,
    ) -> Self {
        Self {
            component,
            stage,
            outcome,
            code,
            command: None,
            selector: None,
        }
    }

    /// Adds only an allowlisted Mesa command code, never header values or payload.
    #[must_use]
    pub const fn with_command(mut self, code: u16) -> Self {
        self.command = match code {
            0x02..=0x04
            | 0x0c..=0x0e
            | 0x1d
            | 0x20
            | 0x27..=0x28
            | 0x2e..=0x2f
            | 0x31
            | 0x38
            | 0x3a
            | 0x3c..=0x40
            | 0x42..=0x44
            | 0x47..=0x49
            | 0x4c => Some(code),
            _ => None,
        };
        self
    }

    /// Adds only a supported keystore operation selector, never a handle or ID.
    #[must_use]
    pub const fn with_keystore_selector(mut self, selector: u8) -> Self {
        self.selector = match selector {
            0x01..=0x04 | 0x06 | 0x0d | 0x18..=0x19 | 0x21 | 0x23..=0x24 | 0x2a | 0x4d => {
                Some(selector)
            }
            _ => None,
        };
        self
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "t1bridge-diagnostic v=1 component={} phase={} result={} code=",
            self.component.label(),
            self.stage.label(),
            self.outcome.label()
        )?;
        match self.code {
            Some(code) => write!(f, "{code}")?,
            None => f.write_str("none")?,
        }
        match self.command {
            Some(code) => write!(f, " command=0x{code:02x}"),
            None => f.write_str(" command=none"),
        }?;
        if let Some(selector) = self.selector {
            write!(f, " selector=0x{selector:02x}")?;
        }
        Ok(())
    }
}

/// Writes at most 4096 records plus one limit marker per process. I/O errors are
/// ignored: diagnostic output must never replace an operation's result.
pub fn emit(record: Record) {
    if !enabled() {
        return;
    }
    write_record(&RECORDS, &mut std::io::stderr().lock(), record);
}

fn write_record(counter: &AtomicUsize, output: &mut impl Write, record: Record) {
    let count = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        (count <= RECORD_LIMIT).then_some(count + 1)
    });
    match count {
        Ok(count) if count < RECORD_LIMIT => {
            let _ = writeln!(output, "{record}");
        }
        Ok(_) => {
            let limit = Record::new(record.component, Stage::Limit, Outcome::Limit, None);
            let _ = writeln!(output, "{limit}");
        }
        Err(_) => {}
    }
}

/// Records a stage around exactly one call without inspecting its value/error.
///
/// # Errors
/// Returns the operation's original error unchanged.
pub fn observe<T, E>(
    component: Component,
    stage: Stage,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if !enabled() {
        return operation();
    }
    emit(Record::new(component, stage, Outcome::Begin, None));
    let result = operation();
    emit(Record::new(
        component,
        stage,
        if result.is_ok() {
            Outcome::Ok
        } else {
            Outcome::Error
        },
        None,
    ));
    result
}

/// Emits a numeric adapter return code before the caller maps it to a category.
pub fn native(component: Component, stage: Stage, code: i32) {
    emit(Record::new(
        component,
        stage,
        if code == 0 {
            Outcome::Ok
        } else {
            Outcome::Error
        },
        Some(i64::from(code)),
    ));
}

/// Reports bounded native cleanup separately from a preserved primary failure.
#[cfg(feature = "sep-operation")]
pub(crate) fn sep_cleanup(status: i32) {
    if let Some(record) = cleanup_record(status) {
        emit(record);
    }
}

#[cfg(any(feature = "sep-operation", test))]
fn cleanup_record(status: i32) -> Option<Record> {
    matches!(status, 0 | -109).then(|| {
        Record::new(
            Component::Sep,
            Stage::SepCleanup,
            if status == 0 {
                Outcome::Ok
            } else {
                Outcome::Error
            },
            Some(i64::from(status)),
        )
    })
}

/// Preserves native reply status without reading keybag handles or payloads.
#[cfg(feature = "sep-operation")]
pub(crate) fn keystore_reply(selector: u8, status: i32, outer: i8, inner: i32) {
    for record in keystore_records(selector, status, outer, inner)
        .into_iter()
        .flatten()
    {
        emit(record);
    }
}

#[cfg(any(feature = "sep-operation", test))]
fn keystore_records(selector: u8, status: i32, outer: i8, inner: i32) -> [Option<Record>; 3] {
    let record = |stage, code: i32| {
        Record::new(
            Component::Sep,
            stage,
            if code == 0 {
                Outcome::Ok
            } else {
                Outcome::Error
            },
            Some(i64::from(code)),
        )
        .with_keystore_selector(selector)
    };
    [
        Some(record(Stage::KeystoreReply, status)),
        (status == 1).then(|| record(Stage::KeystoreOuter, i32::from(outer))),
        (status == 1 && outer == 0).then(|| record(Stage::KeystoreInner, inner)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_records_only_report_the_allowlisted_outcome() {
        for (status, outcome) in [(0, "ok"), (-109, "error")] {
            assert_eq!(
                cleanup_record(status).unwrap().to_string(),
                format!(
                    "t1bridge-diagnostic v=1 component=sep phase=sep-cleanup result={outcome} code={status} command=none"
                )
            );
        }
        for unknown in [1, -103, i32::MIN, i32::MAX] {
            assert!(cleanup_record(unknown).is_none());
        }
    }

    #[test]
    fn keystore_status_never_invents_absent_remote_fields() {
        let lines = |status, outer, inner| {
            keystore_records(3, status, outer, inner)
                .into_iter()
                .flatten()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(0, -5, -7).len(), 1);
        assert_eq!(lines(-3, -5, -7).len(), 1);
        let outer = lines(1, -5, -7);
        assert_eq!(outer.len(), 2);
        assert!(outer[1].contains("phase=keystore-outer result=error code=-5"));
        assert!(outer.iter().all(|s| s.ends_with("selector=0x03")));
        let inner = lines(1, 0, -7);
        assert_eq!(inner.len(), 3);
        assert!(inner[2].contains("phase=keystore-inner result=error code=-7"));
        let rejected = keystore_records(0xff, 1, 0, -7);
        assert!(
            rejected
                .into_iter()
                .flatten()
                .all(|r| !r.to_string().contains("selector="))
        );
    }

    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn output_is_bounded_and_write_failure_is_nonfatal() {
        let counter = AtomicUsize::new(0);
        let mut output = Vec::new();
        let record = Record::new(Component::Efi, Stage::EfiDiscovery, Outcome::Error, Some(5));
        for _ in 0..RECORD_LIMIT + 20 {
            write_record(&counter, &mut output, record);
        }
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.lines().count(), RECORD_LIMIT + 1);
        assert_eq!(text.matches("phase=limit result=limit").count(), 1);
        assert!(
            text.lines()
                .next()
                .unwrap()
                .contains("phase=efi-discovery result=error code=5")
        );
        assert_eq!(counter.load(Ordering::Relaxed), RECORD_LIMIT + 1);

        write_record(&AtomicUsize::new(0), &mut Broken, record);
    }

    #[test]
    fn opt_in_requires_exact_one() {
        for value in [None, Some(""), Some("0"), Some("true"), Some("PRIVATE")] {
            assert!(!requested(value.map(std::ffi::OsStr::new)));
        }
        assert!(requested(Some(std::ffi::OsStr::new("1"))));
    }

    #[test]
    fn records_have_only_labels_codes_and_allowlisted_commands() {
        let record = Record::new(
            Component::Broker,
            Stage::Transaction,
            Outcome::Error,
            Some(1),
        )
        .with_command(3);
        assert_eq!(
            record.to_string(),
            "t1bridge-diagnostic v=1 component=broker phase=transaction result=error code=1 command=0x03"
        );
        assert!(
            record
                .with_command(0xffff)
                .to_string()
                .ends_with("command=none")
        );
    }

    #[test]
    fn cancellation_records_expose_no_contact_or_request_values() {
        for (stage, label) in [
            (Stage::Overlay, "overlay"),
            (Stage::OverlayPress, "overlay-press"),
            (Stage::TouchIdCancel, "touchid-cancel"),
        ] {
            for outcome in [Outcome::Begin, Outcome::Ok, Outcome::Error] {
                let record = Record::new(Component::Renderer, stage, outcome, None);
                assert_eq!(
                    record.to_string(),
                    format!(
                        "t1bridge-diagnostic v=1 component=renderer phase={label} result={} code=none command=none",
                        outcome.label()
                    )
                );
            }
        }
    }

    #[test]
    fn observation_preserves_private_error_without_display_or_debug() {
        struct PrivateError(&'static str);
        let mut calls = 0;
        let result: Result<(), _> = observe(Component::Importer, Stage::Import, || {
            calls += 1;
            Err(PrivateError("PRIVATE-PATH-AND-PAYLOAD"))
        });
        assert_eq!(calls, 1);
        assert_eq!(result.err().unwrap().0, "PRIVATE-PATH-AND-PAYLOAD");
    }
}
