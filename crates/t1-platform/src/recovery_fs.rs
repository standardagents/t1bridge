//! Descriptor-anchored storage for private attempts and EFI generations.

use std::os::{
    fd::{AsRawFd, BorrowedFd, FromRawFd},
    unix::ffi::OsStrExt,
};
use std::{
    ffi::{CString, c_char, c_int},
    fs::File,
    io,
    path::Path,
};

unsafe extern "C" {
    fn t1_recovery_root(path: *const c_char, esp: c_int, output: *mut c_int) -> c_int;
    fn t1_recovery_child(
        parent: c_int,
        name: *const c_char,
        create: c_int,
        private: c_int,
        output: *mut c_int,
    ) -> c_int;
    fn t1_recovery_lock(fd: c_int) -> c_int;
    fn t1_recovery_exists(fd: c_int, name: *const c_char, exists: *mut c_int) -> c_int;
    fn t1_recovery_file(fd: c_int, name: *const c_char, create: c_int, output: *mut c_int)
    -> c_int;
    fn t1_recovery_rename(
        source: c_int,
        old: *const c_char,
        destination: c_int,
        new: *const c_char,
    ) -> c_int;
    fn t1_recovery_space(fd: c_int, required: u64) -> c_int;
}

fn check(status: c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status))
    }
}

fn name(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(|_| io::ErrorKind::InvalidInput.into())
}

fn owned(status: c_int, fd: c_int) -> io::Result<File> {
    check(status)?;
    if fd < 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    // SAFETY: successful C calls transfer exactly one newly opened descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// A directory held open for the entire transaction. It never follows symlinks.
pub struct Directory(File);

impl Directory {
    /// Opens an existing owner-controlled directory, optionally requiring a FAT ESP.
    ///
    /// # Errors
    /// Rejects symlinks, writable-by-others directories, wrong owners and non-ESPs.
    pub fn open(path: &Path, esp: bool) -> io::Result<Self> {
        let path =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let mut fd = -1;
        // SAFETY: live NUL-terminated path and output pointer.
        let status = unsafe { t1_recovery_root(path.as_ptr(), i32::from(esp), &raw mut fd) };
        owned(status, fd).map(Self)
    }

    /// Opens or creates a direct child, checking ownership and directory modes.
    ///
    /// # Errors
    /// Rejects links, mount crossings and unsafe ownership or permissions.
    pub fn child(&self, child: &str, create: bool, private: bool) -> io::Result<Self> {
        child_directory(self.0.as_raw_fd(), child, create, private)
    }

    /// Exclusively locks the directory for this process's attempt.
    ///
    /// # Errors
    /// Returns an error when another recovery process holds the lock.
    pub fn lock(&self) -> io::Result<()> {
        // SAFETY: the owned directory descriptor is live.
        check(unsafe { t1_recovery_lock(self.0.as_raw_fd()) })
    }

    /// Checks existence without following even the final symlink.
    ///
    /// # Errors
    /// Returns inspection errors separately from absence.
    pub fn exists(&self, child: &str) -> io::Result<bool> {
        let name = name(child)?;
        let mut exists = 0;
        // SAFETY: live descriptor, C string and output pointer.
        check(unsafe { t1_recovery_exists(self.0.as_raw_fd(), name.as_ptr(), &raw mut exists) })?;
        Ok(exists != 0)
    }

    /// Opens a held regular inode or creates a new file exclusively.
    ///
    /// # Errors
    /// Rejects links, special files, wrong owners and existing create targets.
    pub fn file(&self, child: &str, create: bool) -> io::Result<File> {
        let name = name(child)?;
        let mut fd = -1;
        // SAFETY: pointers and owned descriptor remain valid throughout C.
        let status = unsafe {
            t1_recovery_file(
                self.0.as_raw_fd(),
                name.as_ptr(),
                i32::from(create),
                &raw mut fd,
            )
        };
        owned(status, fd)
    }

    /// Renames one child without replacing any existing destination.
    ///
    /// # Errors
    /// Returns conflicts and filesystem failures; no replacement fallback exists.
    pub fn rename(&self, old: &str, destination: &Self, new: &str) -> io::Result<()> {
        let old = name(old)?;
        let new = name(new)?;
        // SAFETY: both directory descriptors and strings are live.
        check(unsafe {
            t1_recovery_rename(
                self.0.as_raw_fd(),
                old.as_ptr(),
                destination.0.as_raw_fd(),
                new.as_ptr(),
            )
        })
    }

    /// Flushes directory metadata after a file or generation change.
    ///
    /// # Errors
    /// Any failure means durability has not been proven.
    pub fn sync(&self) -> io::Result<()> {
        self.0.sync_all()
    }

    /// Requires enough unprivileged free space on a writable filesystem.
    ///
    /// # Errors
    /// Returns read-only, inspection or insufficient-space failures.
    pub fn require_space(&self, bytes: u64) -> io::Result<()> {
        // SAFETY: live owned directory descriptor.
        check(unsafe { t1_recovery_space(self.0.as_raw_fd(), bytes) })
    }
}

/// Opens a direct child of an existing EFI discovery descriptor.
///
/// # Errors
/// Preserves absence, permission and inspection errors distinctly.
pub fn discovered_child(parent: BorrowedFd<'_>, child: &str) -> io::Result<Directory> {
    child_directory(parent.as_raw_fd(), child, false, false)
}

fn child_directory(
    parent: c_int,
    child: &str,
    create: bool,
    private: bool,
) -> io::Result<Directory> {
    let name = name(child)?;
    let mut fd = -1;
    // SAFETY: valid descriptor, NUL-terminated child name and output pointer.
    let status = unsafe {
        t1_recovery_child(
            parent,
            name.as_ptr(),
            i32::from(create),
            i32::from(private),
            &raw mut fd,
        )
    };
    owned(status, fd).map(Directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::{
        fs,
        io::{Read, Write},
    };

    #[test]
    fn held_directories_preserve_conflicts_and_reject_links() {
        let root = std::env::temp_dir().join(format!(
            "t1-recovery-storage-synthetic-{}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let directory = Directory::open(&root, false).unwrap();
        let a = directory.child("a", true, true).unwrap();
        let b = directory.child("b", true, true).unwrap();
        a.file("data", true)
            .unwrap()
            .write_all(b"synthetic original")
            .unwrap();
        b.file("data", true)
            .unwrap()
            .write_all(b"synthetic retained")
            .unwrap();
        assert!(a.rename("data", &b, "data").is_err());
        let mut contents = String::new();
        b.file("data", false)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "synthetic retained");
        symlink(root.join("b/data"), root.join("a/link")).unwrap();
        assert!(a.file("link", false).is_err());
        assert!(directory.child("../escape", true, true).is_err());
        a.rename("data", &b, "new").unwrap();
        assert!(!a.exists("data").unwrap());
        assert!(b.exists("new").unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
