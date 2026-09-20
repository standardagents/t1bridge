//! Attended, native recovery of missing T1 firmware and factory data.
//!
//! No recovery executable, third-party restore stack, or background network
//! service participates in this path. Network and USB work require an explicit
//! command; importing preserved local data never invokes online recovery.

mod fdr_proxy;
mod firmware;
mod mux;
mod plist;
mod restore;
mod signing;
mod storage;
mod usb;

use std::io::{IsTerminal, Read, Write};
use std::os::{
    fd::AsFd,
    unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
};
use std::path::Path;
use std::time::Duration;
use t1_platform::recovery_io;

/// Runs an explicit, attended online recovery only when preserved EFI is absent.
///
/// Existing local data and all inspection errors block online recovery. The
/// caller must be root, provide a mounted FAT ESP, and acknowledge the physical
/// restore on a terminal. Experimental hardware acceptance remains separate.
///
/// # Errors
/// Returns an identifier-free error at the first failed check or operation.
/// Private incomplete attempts are retained; no artifact makes failure success.
pub fn run(efi: &Path) -> Result<()> {
    if !t1_platform::preserved_efi_discovery::is_root() {
        return Err(Error("online recovery requires root authority"));
    }
    usb::supported_machine()?;
    storage::require_local_absence()?;
    let device = usb::discover()?.ok_or(Error("no T1 recovery device is present"))?;
    if device.product != usb::RECOVERY || device.has_hid {
        return Err(Error(
            "online recovery requires a T1 already in recovery mode",
        ));
    }
    let reset = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(0o400_000)
        .open("/dev/t1bridge-recovery")
        .map_err(|_| {
            Error("native reset interface unavailable; install matching T1Bridge DKMS and reboot")
        })?;
    let metadata = reset.metadata()?;
    if !metadata.file_type().is_char_device() || metadata.uid() != 0 || metadata.mode() & 0o077 != 0
    {
        return Err(Error(
            "native reset interface has unsafe ownership or permissions",
        ));
    }
    recovery_io::reset(reset.as_fd(), device.bus, device.address, false)
        .map_err(|_| Error("a unique T1-only ACPI reset method could not be validated"))?;
    let destination = storage::EfiDestination::open(efi)?;
    confirm()?;
    println!("recovery: downloading and verifying Apple's pinned firmware");
    let firmware = firmware::Firmware::download()?;
    storage::require_local_absence()?;
    let attempt = storage::Attempt::create()?;
    let provision = attempt.phase("provision")?;
    println!("recovery: provisioning factory data");
    let (first, ecid) = restore::run(&device, None, &firmware, None, |name, bytes| {
        storage::save(&provision, name, bytes)
    })?;
    storage::save(&provision, "complete", b"restore-success\n")?;
    let location = first.device.location.clone();
    let reset_device = || -> Result<usb::Device> {
        // The service can re-enumerate after its final status. Resolve the
        // current address at the same physical location before issuing FRST.
        let device = usb::wait(&location, usb::BOOTED, None, Duration::from_secs(10))?;
        recovery_io::reset(reset.as_fd(), device.bus, device.address, true)?;
        let next = usb::wait(&location, usb::RECOVERY, None, Duration::from_secs(60))?;
        std::thread::sleep(Duration::from_secs(12));
        Ok(next)
    };
    let device = reset_device()?;
    let personalize = attempt.phase("personalize")?;
    println!("recovery: personalizing firmware with the recovered factory data");
    let (second, _) = restore::run(
        &device,
        Some(ecid),
        &firmware,
        Some(&first.fdr),
        |name, bytes| storage::save(&personalize, name, bytes),
    )?;
    storage::save(&personalize, "complete", b"restore-success-and-fdr-match\n")?;
    storage::save(&personalize, "version.plist", &firmware.version)?;
    let device = reset_device()?;
    println!("recovery: booting and checking the recovered firmware");
    restore::boot(&device, ecid, &second.signed)?;
    storage::save(
        &personalize,
        "boot-verified",
        b"functional-usb-stable-30s\n",
    )?;
    destination.install(&first.fdr, &second.signed.combined, &firmware.version)?;
    attempt.finish()?;
    println!("recovery: EFI generation saved; importing sensor-matched local data");
    crate::runtime::attempt_protected_import()
        .map_err(|_| Error("EFI recovery completed; local machine-data import needs attention"))?;
    println!("recovery: complete; verify status, Touch ID, and a later warm and cold boot");
    Ok(())
}

fn confirm() -> Result<()> {
    let mut terminal = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| Error("online recovery requires an attended terminal"))?;
    if !terminal.is_terminal() {
        return Err(Error("online recovery requires an attended terminal"));
    }
    terminal.write_all(b"Experimental T1 online recovery. Apple will receive this T1's identity and signing nonces.\nThis runs two firmware restores and saves recovered EFI data. Keep the Mac powered and awake.\nUse preserved local data or a verified same-Mac backup first.\nType RECOVER after confirming neither is available: ")?;
    terminal.flush()?;
    let mut answer = Vec::new();
    for _ in 0..64 {
        let mut byte = [0];
        if terminal.read(&mut byte)? == 0 {
            return Err(Error("recovery confirmation cancelled"));
        }
        if byte[0] == b'\n' {
            break;
        }
        answer.push(byte[0]);
    }
    if answer != b"RECOVER" {
        return Err(Error("recovery confirmation cancelled"));
    }
    Ok(())
}

/// A deliberately value-free diagnostic: device identities, tickets and FDR
/// contents must never escape through an error or a debug representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error(pub(crate) &'static str);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self("recovery input/output failed")
    }
}

type Result<T> = std::result::Result<T, Error>;
