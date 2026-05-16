//! Daemon-side helper to create a per-activity shm region for the cef_host
//! BGRA ring. Returns an `OwnedFd` sized via `ShmReader::required_region_size`
//! (which matches `ShmWriter::required_region_size` on the cef_host side); the
//! caller is responsible for sending the fd to cef_host via SCM_RIGHTS.

use crate::shm_reader::ShmReader;
use std::os::fd::OwnedFd;

/// Default PoC payload budget: 1280×800 BGRA + 4 KiB slack for damage rects /
/// metadata. Larger viewports go through the same constant in this PoC and
/// get clipped on the writer side.
pub const POC_SLOT_PAYLOAD_MAX: usize = 1280 * 800 * 4 + 4096;

/// Creates a shm fd sized for one activity's screencast ring.
pub fn create_shm_for_activity(aid: &str, slot_payload_max: usize) -> std::io::Result<OwnedFd> {
    let total = ShmReader::required_region_size(slot_payload_max);
    create_shm_region(aid, total)
}

#[cfg(target_os = "linux")]
fn create_shm_region(aid: &str, total: usize) -> std::io::Result<OwnedFd> {
    use nix::sys::memfd::{MemFdCreateFlag, memfd_create};
    let name = std::ffi::CString::new(format!("ozmux-{aid}"))
        .map_err(|e| std::io::Error::other(format!("aid contains NUL: {e}")))?;
    let fd = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC)?;
    nix::unistd::ftruncate(&fd, total as i64)?;
    Ok(fd)
}

#[cfg(target_os = "macos")]
fn create_shm_region(aid: &str, total: usize) -> std::io::Result<OwnedFd> {
    use std::os::fd::FromRawFd;
    // NOTE: macOS shm names are capped at PSHMNAMLEN (31 chars including
    // leading slash). Trim the activity id so the formatted name stays in
    // bounds even when the caller passes a long uuid.
    let safe_aid: String = aid.chars().take(20).collect();
    let path = std::ffi::CString::new(format!("/ozmux-{safe_aid}"))
        .map_err(|e| std::io::Error::other(format!("aid contains NUL: {e}")))?;
    // SAFETY: shm_unlink is idempotent when the name does not exist.
    unsafe {
        libc::shm_unlink(path.as_ptr());
    }
    // SAFETY: shm_open with O_CREAT | O_EXCL creates a fresh region after the
    // pre-emptive unlink above; result is checked below.
    let fd = unsafe {
        libc::shm_open(
            path.as_ptr(),
            libc::O_CREAT | libc::O_RDWR | libc::O_EXCL,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: ftruncate sets the shm object size on the newly opened fd.
    if unsafe { libc::ftruncate(fd, total as libc::off_t) } < 0 {
        let err = std::io::Error::last_os_error();
        // SAFETY: closing the fd we just opened to avoid a leak on failure.
        unsafe {
            libc::close(fd);
        }
        // SAFETY: unlink the name we just created so subsequent retries succeed.
        unsafe {
            libc::shm_unlink(path.as_ptr());
        }
        return Err(err);
    }
    // SAFETY: unlinking the name immediately keeps the region alive via the fd
    // only — no other process can open it by name.
    unsafe {
        libc::shm_unlink(path.as_ptr());
    }
    // SAFETY: shm_open returned a valid fd; transferring ownership to OwnedFd.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd as _;

    #[test]
    fn create_shm_returns_valid_fd() {
        let fd = create_shm_for_activity("test-aid", POC_SLOT_PAYLOAD_MAX)
            .expect("create_shm_for_activity failed");
        assert!(fd.as_raw_fd() >= 0);
    }
}
