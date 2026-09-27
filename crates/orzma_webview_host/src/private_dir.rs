//! Restricting a runtime directory to the current user: `chmod 0700` on
//! Unix, a protected, inheritable current-user DACL on Windows.

use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
#[cfg(windows)]
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetFileSecurityW, GetTokenInformation, PSECURITY_DESCRIPTOR,
    SetFileSecurityW, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Makes `path` readable, writable, and listable only by the current user.
///
/// Unix: `chmod 0700`. Windows: a protected DACL whose single
/// full-control entry names the current user and is inheritable, so files
/// created inside afterwards are covered too.
#[cfg(unix)]
pub fn restrict_to_current_user(path: &Path) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Makes `path` readable, writable, and listable only by the current user.
///
/// Unix: `chmod 0700`. Windows: a protected DACL whose single
/// full-control entry names the current user and is inheritable, so files
/// created inside afterwards are covered too.
#[cfg(windows)]
pub fn restrict_to_current_user(path: &Path) -> io::Result<()> {
    let sid = current_user_sid()?;
    let sddl = wide(&format!("D:P(A;OICI;FA;;;{sid})"));
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated UTF-16 that outlives the call, and
    // `descriptor` is a valid out-pointer; the result is freed by `LocalOwned`.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = LocalOwned(descriptor.cast());
    let path = wide_path(path);
    // SAFETY: `path` is NUL-terminated UTF-16 and `descriptor.0` is a valid
    // self-relative security descriptor for the duration of the call.
    let ok = unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            descriptor.0.cast(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The current user's SID in string form (`S-1-5-21-…`).
#[cfg(windows)]
pub fn current_user_sid() -> io::Result<String> {
    let mut raw: HANDLE = std::ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
    // closing; `raw` is a valid out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a real token handle the call above just opened and
    // nothing else owns it.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut len = 0u32;
    // SAFETY: a null buffer with length 0 is the documented way to query the
    // required size; the call fails with ERROR_INSUFFICIENT_BUFFER by design.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut len,
        );
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` holds `len` writable bytes and outlives the call.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call filled `buf` with a TOKEN_USER; `read_unaligned` is
    // used because a `Vec<u8>` guarantees only byte alignment.
    let user = unsafe { buf.as_ptr().cast::<TOKEN_USER>().read_unaligned() };
    let mut sid_str: *mut u16 = std::ptr::null_mut();
    // SAFETY: `user.User.Sid` points inside `buf`, which is still alive;
    // `sid_str` is a valid out-pointer freed by `LocalOwned`.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_str) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = LocalOwned(sid_str.cast());
    // SAFETY: `sid_str` is a NUL-terminated UTF-16 string the API allocated.
    Ok(unsafe { wide_to_string(owned.0.cast()) })
}

/// The DACL of `path` rendered as an SDDL string (`D:…`).
#[cfg(windows)]
pub fn security_descriptor_sddl(path: &Path) -> io::Result<String> {
    let path = wide_path(path);
    let mut len = 0u32;
    // SAFETY: a null buffer with length 0 queries the required size.
    unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut len,
        );
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` holds `len` writable bytes and outlives the call.
    let ok = unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out: *mut u16 = std::ptr::null_mut();
    let mut out_len = 0u32;
    // SAFETY: `buf` holds a self-relative security descriptor the call above
    // wrote; `out` is a valid out-pointer freed by `LocalOwned`.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            buf.as_mut_ptr().cast(),
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut out,
            &mut out_len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = LocalOwned(out.cast());
    // SAFETY: `out` is a NUL-terminated UTF-16 string the API allocated.
    Ok(unsafe { wide_to_string(owned.0.cast()) })
}

/// Normalizes an SDDL string the way Windows renders it, so well-known SIDs
/// come out as their two-letter aliases (for example `LA` for the built-in
/// Administrator) exactly as [`security_descriptor_sddl`] reports them.
#[cfg(windows)]
pub fn canonical_sddl(sddl: &str) -> io::Result<String> {
    let wide_sddl = wide(sddl);
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `wide_sddl` is NUL-terminated UTF-16 that outlives the call, and
    // `descriptor` is a valid out-pointer; the result is freed by `LocalOwned`.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide_sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = LocalOwned(descriptor.cast());
    let mut out: *mut u16 = std::ptr::null_mut();
    let mut out_len = 0u32;
    // SAFETY: `descriptor.0` is a valid self-relative security descriptor for
    // the duration of the call; `out` is a valid out-pointer freed by `LocalOwned`.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0.cast(),
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut out,
            &mut out_len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = LocalOwned(out.cast());
    // SAFETY: `out` is a NUL-terminated UTF-16 string the API allocated.
    Ok(unsafe { wide_to_string(owned.0.cast()) })
}

/// Asserts that `path` is private to the current user, for this crate's
/// tests.
#[cfg(all(test, unix))]
pub(crate) fn assert_private_dir(path: &Path) {
    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

/// Asserts that `path` is private to the current user, for this crate's
/// tests.
#[cfg(all(test, windows))]
pub(crate) fn assert_private_dir(path: &Path) {
    let sddl = security_descriptor_sddl(path).unwrap();
    let sid = current_user_sid().unwrap();
    let expected = canonical_sddl(&format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
    assert_eq!(
        sddl, expected,
        "the directory must carry exactly one inheritable current-user ACE"
    );
}

/// A `LocalAlloc`-owned pointer, freed on drop.
#[cfg(windows)]
struct LocalOwned(HLOCAL);

#[cfg(windows)]
impl Drop for LocalOwned {
    fn drop(&mut self) {
        // SAFETY: the pointer came from an API that allocates with LocalAlloc
        // and is freed exactly once, here.
        unsafe {
            LocalFree(self.0);
        }
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Reads a NUL-terminated UTF-16 string.
///
/// # Safety
///
/// `ptr` must point to a valid, NUL-terminated UTF-16 buffer.
#[cfg(windows)]
unsafe fn wide_to_string(ptr: *const u16) -> String {
    let mut len = 0usize;
    // SAFETY: the caller guarantees a NUL terminator, so every read before it
    // is in bounds.
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` u16s before the terminator are in bounds per the caller.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a restricted directory is readable only by the
    /// current user.
    ///
    /// Case: the control-socket directory is created under a temp root
    /// other users can list.
    #[test]
    fn a_restricted_dir_is_private_to_the_current_user() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sock");
        std::fs::create_dir(&target).unwrap();
        restrict_to_current_user(&target).unwrap();
        assert_private_dir(&target);
    }

    /// Asserts that a file created inside a restricted directory inherits
    /// the restriction.
    ///
    /// Case: `bind` creates the socket file after the directory was
    /// restricted.
    #[cfg(windows)]
    #[test]
    fn a_file_created_inside_a_restricted_dir_inherits_the_restriction() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sock");
        std::fs::create_dir(&target).unwrap();
        restrict_to_current_user(&target).unwrap();
        let file = target.join("control.sock");
        std::fs::write(&file, b"").unwrap();
        let sddl = security_descriptor_sddl(&file).unwrap();
        let sid = current_user_sid().unwrap();
        let expected = canonical_sddl(&format!("D:(A;;FA;;;{sid})")).unwrap();
        assert_eq!(
            sddl, expected,
            "the file must carry exactly the inherited current-user ACE"
        );
    }
}
