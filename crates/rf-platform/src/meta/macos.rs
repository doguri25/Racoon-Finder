#![allow(unsafe_code, reason = "lchflags FFI")]

//! The macOS `UF_HIDDEN` flag, which hides an item in Finder.

use std::ffi::CString;
use std::fs::Metadata;
use std::io;
use std::os::macos::fs::MetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Sets `UF_HIDDEN` on `local` if `hidden`, clears it otherwise, without following a final
/// link. The other flags stay as they are, and nothing is written if the flag is already right.
pub(super) fn set_hidden_flag(local: &Path, hidden: bool) -> io::Result<()> {
    let flags = std::fs::symlink_metadata(local)?.st_flags();
    let wanted = if hidden {
        flags | libc::UF_HIDDEN
    } else {
        flags & !libc::UF_HIDDEN
    };
    if wanted == flags {
        return Ok(());
    }
    let path = CString::new(local.as_os_str().as_bytes())?;
    // SAFETY: `path` is NUL-terminated and outlives the call.
    if unsafe { libc::lchflags(path.as_ptr(), wanted) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Gives `target` the `UF_HIDDEN` flag of the item whose metadata is `source`.
pub(super) fn copy_hidden_flag(source: &Metadata, target: &Path) -> io::Result<()> {
    set_hidden_flag(target, source.st_flags() & libc::UF_HIDDEN != 0)
}
