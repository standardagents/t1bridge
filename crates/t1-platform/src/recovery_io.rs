//! Safe ownership and size checks for the attended recovery C boundary.
//!
//! The only HTTPS destinations are the pinned Apple firmware package and TSS.
//! USB calls operate on a caller-owned descriptor; no USB daemon or listener is
//! created. Library errors are intentionally stripped of response contents.

use std::ffi::{CString, c_char, c_int, c_uint};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

unsafe extern "C" {
    fn t1_recovery_https(
        url: *const c_char,
        request: *const u8,
        request_size: usize,
        output: *mut u8,
        capacity: usize,
        output_size: *mut usize,
    ) -> c_int;
    fn t1_recovery_sha256(input: *const u8, size: usize, output: *mut u8) -> c_int;
    fn t1_recovery_xar(
        input: *const u8,
        size: usize,
        name: *const c_char,
        output: *mut u8,
        capacity: usize,
        output_size: *mut usize,
    ) -> c_int;
    fn t1_recovery_usb_claim(fd: c_int, interface: c_uint) -> c_int;
    fn t1_recovery_usb_release(fd: c_int, interface: c_uint) -> c_int;
    fn t1_recovery_usb_configuration(fd: c_int, configuration: c_uint) -> c_int;
    fn t1_recovery_reset(fd: c_int, bus: u32, address: u32, execute: c_int) -> c_int;
    fn t1_recovery_usb_control(
        fd: c_int,
        kind: u8,
        request: u8,
        value: u16,
        index: u16,
        bytes: *mut u8,
        size: u16,
        timeout: c_uint,
        actual: *mut usize,
    ) -> c_int;
    fn t1_recovery_usb_bulk(
        fd: c_int,
        endpoint: c_uint,
        bytes: *mut u8,
        size: usize,
        timeout: c_uint,
        actual: *mut usize,
    ) -> c_int;
}

fn checked(status: c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status))
    }
}

/// Fixed, independently bounded Apple endpoints used during explicit recovery.
#[derive(Clone, Copy, Debug)]
pub enum AppleEndpoint {
    Firmware,
    Signing,
}

/// Fetches firmware or submits a signing request over verified HTTPS.
///
/// # Errors
/// Returns an error on certificate, HTTP, network, timeout or size failure.
pub fn https(endpoint: AppleEndpoint, request: Option<&[u8]>) -> io::Result<Vec<u8>> {
    let (url, limit) = match endpoint {
        AppleEndpoint::Firmware => (c"https://swcdn.apple.com/content/downloads/22/59/001-72525-A_7H83CSQW4K/p9dd3a0vdtdssud9qlxd4i73pn389rxugu/EmbeddedOSFirmware.pkg", 59_314_427),
        AppleEndpoint::Signing => (c"https://gs.apple.com/TSS/controller?action=2", 16 * 1024 * 1024),
    };
    if matches!(endpoint, AppleEndpoint::Firmware) != request.is_none() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut output = vec![0; limit];
    let mut used = 0;
    let (pointer, length) = request.map_or((std::ptr::null(), 0), |v| (v.as_ptr(), v.len()));
    // SAFETY: all pointers refer to live slices; C bounds every write to capacity.
    checked(unsafe {
        t1_recovery_https(
            url.as_ptr(),
            pointer,
            length,
            output.as_mut_ptr(),
            output.len(),
            &raw mut used,
        )
    })?;
    if used > limit {
        return Err(io::ErrorKind::InvalidData.into());
    }
    output.truncate(used);
    Ok(output)
}

/// Computes a SHA-256 digest using the system OpenSSL provider.
///
/// # Errors
/// Returns an error if the crypto provider is unavailable.
pub fn sha256(input: &[u8]) -> io::Result<[u8; 32]> {
    let mut output = [0; 32];
    // SAFETY: input and fixed-size output remain valid for the synchronous call.
    checked(unsafe { t1_recovery_sha256(input.as_ptr(), input.len(), output.as_mut_ptr()) })?;
    Ok(output)
}

/// Reads a unique regular member from a XAR without extracting filesystem paths.
///
/// # Errors
/// Rejects malformed archives, duplicate members, links and oversized output.
pub fn xar_member(input: &[u8], name: &str, limit: usize) -> io::Result<Vec<u8>> {
    if limit > 64 * 1024 * 1024 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let name = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut output = vec![0; limit];
    let mut used = 0;
    // SAFETY: live slices and NUL-terminated name; C checks both buffer limits.
    checked(unsafe {
        t1_recovery_xar(
            input.as_ptr(),
            input.len(),
            name.as_ptr(),
            output.as_mut_ptr(),
            output.len(),
            &raw mut used,
        )
    })?;
    if used > limit {
        return Err(io::ErrorKind::InvalidData.into());
    }
    output.truncate(used);
    Ok(output)
}

/// Claims an interface without detaching a kernel driver.
///
/// # Errors
/// Returns the kernel error when an interface is busy or unavailable.
pub fn usb_claim(fd: BorrowedFd<'_>, interface: u8) -> io::Result<()> {
    // SAFETY: the borrowed descriptor remains open throughout ioctl.
    checked(unsafe { t1_recovery_usb_claim(fd.as_raw_fd(), interface.into()) })
}

/// Releases an interface claimed by this descriptor.
pub fn usb_release(fd: BorrowedFd<'_>, interface: u8) {
    // SAFETY: the borrowed descriptor remains open throughout ioctl.
    let _ = unsafe { t1_recovery_usb_release(fd.as_raw_fd(), interface.into()) };
}

/// Selects a descriptor-validated USB configuration for an unconfigured device.
///
/// # Errors
/// Returns the kernel error if configuration selection fails.
pub fn usb_configuration(fd: BorrowedFd<'_>, configuration: u8) -> io::Result<()> {
    // SAFETY: the descriptor stays open throughout the synchronous ioctl.
    checked(unsafe { t1_recovery_usb_configuration(fd.as_raw_fd(), configuration.into()) })
}

/// Checks or invokes the kernel's model- and personality-guarded T1-only reset.
///
/// # Errors
/// Fails for working devices, ambiguous ACPI methods, wrong models or missing authority.
pub fn reset(fd: BorrowedFd<'_>, bus: u32, address: u32, execute: bool) -> io::Result<()> {
    // SAFETY: the owned device descriptor remains valid for this ioctl.
    checked(unsafe { t1_recovery_reset(fd.as_raw_fd(), bus, address, i32::from(execute)) })
}

/// One bounded USB control request.
#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub kind: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

/// Performs a control transfer with a finite timeout.
///
/// # Errors
/// Rejects oversized buffers and propagates transport errors.
pub fn usb_control(
    fd: BorrowedFd<'_>,
    control: Control,
    bytes: &mut [u8],
    timeout_ms: u32,
) -> io::Result<usize> {
    let size = u16::try_from(bytes.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut actual = 0;
    // SAFETY: slice length is checked; the kernel bounds writes by wLength.
    checked(unsafe {
        t1_recovery_usb_control(
            fd.as_raw_fd(),
            control.kind,
            control.request,
            control.value,
            control.index,
            bytes.as_mut_ptr(),
            size,
            timeout_ms.clamp(1, 30_000),
            &raw mut actual,
        )
    })?;
    Ok(actual)
}

/// Performs a bounded bulk transfer with a finite timeout.
///
/// # Errors
/// Returns an error on oversized buffers or transport failures.
pub fn usb_bulk(
    fd: BorrowedFd<'_>,
    endpoint: u8,
    bytes: &mut [u8],
    timeout_ms: u32,
) -> io::Result<usize> {
    let mut actual = 0;
    // SAFETY: C bounds the supplied length; the kernel bounds writes by len.
    checked(unsafe {
        t1_recovery_usb_bulk(
            fd.as_raw_fd(),
            endpoint.into(),
            bytes.as_mut_ptr(),
            bytes.len(),
            timeout_ms.clamp(1, 30_000),
            &raw mut actual,
        )
    })?;
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_known_vector() {
        assert_eq!(
            sha256(b"abc").unwrap(),
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad
            ]
        );
    }

    #[test]
    fn malformed_archive_and_wrong_endpoint_usage_fail_without_network() {
        assert!(xar_member(b"not an archive", "Payload", 1024).is_err());
        assert!(https(AppleEndpoint::Firmware, Some(b"private")).is_err());
        assert!(https(AppleEndpoint::Signing, None).is_err());
    }
}
