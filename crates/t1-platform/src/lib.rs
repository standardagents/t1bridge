//! Audited safe wrappers around `T1Bridge`'s focused Linux C boundaries.

pub mod diagnostics;
#[allow(unsafe_code)]
mod ffi;

#[cfg(feature = "online-recovery")]
#[allow(unsafe_code)]
pub mod recovery_io;

#[cfg(feature = "online-recovery")]
#[allow(unsafe_code)]
pub mod recovery_fs;

#[cfg(feature = "frame-memfd")]
pub mod frame_memfd;
#[cfg(feature = "import-fs")]
pub mod import_fs;
#[cfg(feature = "preserved-efi")]
pub mod preserved_efi;
#[cfg(feature = "preserved-efi-discovery")]
pub mod preserved_efi_discovery;
#[cfg(feature = "secret-wipe")]
pub mod secret;
#[cfg(feature = "sep-operation")]
pub mod sep;
#[cfg(feature = "seqpacket")]
pub mod seqpacket;
#[cfg(feature = "system-brightness")]
pub mod system_brightness;
#[cfg(feature = "touchbar-drm")]
pub mod touchbar_drm;
#[cfg(feature = "touchbar-io")]
pub mod touchbar_io;
#[cfg(feature = "touchbar-session")]
pub mod touchbar_session;
#[cfg(feature = "usb-cycle-guard")]
pub mod usb_cycle_guard;
#[cfg(feature = "xz")]
pub mod xz;
