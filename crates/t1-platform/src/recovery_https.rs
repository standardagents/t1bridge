//! Owned, verified TLS transport for the legacy FDR bootstrap destination.
//! The caller validates DNS addresses. C connects only to the supplied numeric
//! address on port 443 while authenticating the fixed hostname gg.apple.com.

use std::ffi::{CString, c_char, c_int, c_void};
use std::io::{self, Read, Write};
use std::net::IpAddr;
use std::ptr::NonNull;

unsafe extern "C" {
    fn t1_fdr_https_open(address: *const c_char, output: *mut *mut c_void) -> c_int;
    fn t1_fdr_https_read(
        handle: *mut c_void,
        bytes: *mut u8,
        size: usize,
        actual: *mut usize,
    ) -> c_int;
    fn t1_fdr_https_write(
        handle: *mut c_void,
        bytes: *const u8,
        size: usize,
        actual: *mut usize,
    ) -> c_int;
    fn t1_fdr_https_close(handle: *mut c_void);
}

fn checked(status: c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status))
    }
}

/// A single TLS connection; ownership never exposes a raw socket or plaintext fallback.
pub struct FdrHttps(NonNull<c_void>);

impl FdrHttps {
    /// Connects to an already validated public address on HTTPS port 443.
    ///
    /// # Errors
    /// Fails on TLS chain/hostname validation, connection, timeout or allocation failure.
    pub fn connect(address: IpAddr) -> io::Result<Self> {
        let address = CString::new(address.to_string()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let mut output = std::ptr::null_mut();
        // SAFETY: address and output live through the synchronous call; C returns
        // ownership of a verified connection only on success.
        checked(unsafe { t1_fdr_https_open(address.as_ptr(), &raw mut output) })?;
        NonNull::new(output)
            .map(Self)
            .ok_or(io::ErrorKind::InvalidData.into())
    }
}

impl Read for FdrHttps {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = bytes.len().min(1024 * 1024);
        let mut used = 0;
        // SAFETY: this handle is owned and live; the slice bounds the C write.
        checked(unsafe {
            t1_fdr_https_read(self.0.as_ptr(), bytes.as_mut_ptr(), count, &raw mut used)
        })?;
        if used > count {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(used)
    }
}

impl Write for FdrHttps {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = bytes.len().min(1024 * 1024);
        let mut used = 0;
        // SAFETY: this handle is owned and live; the input slice covers count bytes.
        checked(unsafe {
            t1_fdr_https_write(self.0.as_ptr(), bytes.as_ptr(), count, &raw mut used)
        })?;
        if used > count {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(used)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for FdrHttps {
    fn drop(&mut self) {
        // SAFETY: this object uniquely owns the handle, which is freed exactly once.
        unsafe { t1_fdr_https_close(self.0.as_ptr()) };
    }
}
