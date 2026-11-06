#![allow(
    unsafe_code,
    reason = "NSURL's bookmark resolution takes a raw pointer for its stale flag"
)]

//! Links on macOS: a Finder alias is a bookmark file. It is made from the bookmark data of the
//! target (`NSURLBookmarkCreationSuitableForBookmarkFile`) written to the alias's URL with
//! `+[NSURL writeBookmarkData:toURL:options:error:]`, and read by reading the bookmark data back
//! and resolving it without UI and without mounting anything.
//!
//! Both URLs are the file URLs of `crate::clipboard::file_url`, byte for byte as the path is
//! written (`NSURL`'s own path conversions would decompose a name).

use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::autoreleasepool;
use objc2::runtime::Bool;
use objc2_foundation::{
    NSError, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
};

use crate::clipboard::file_url;

/// `NSFileNoSuchFileError`, `NSFileReadNoSuchFileError`, `NSFileWriteFileExistsError`,
/// `NSFileWriteNoPermissionError` and `NSFileReadNoPermissionError` of `NSCocoaErrorDomain`.
const NO_SUCH_FILE: isize = 4;
const READ_NO_SUCH_FILE: isize = 260;
const FILE_EXISTS: isize = 516;
const WRITE_NO_PERMISSION: isize = 513;
const READ_NO_PERMISSION: isize = 257;

/// The `io::Error` of a Foundation error: the kinds the jobs tell apart, the rest as text.
fn io_error(error: &NSError) -> io::Error {
    let kind = match error.code() {
        NO_SUCH_FILE | READ_NO_SUCH_FILE => io::ErrorKind::NotFound,
        FILE_EXISTS => io::ErrorKind::AlreadyExists,
        WRITE_NO_PERMISSION | READ_NO_PERMISSION => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error.localizedDescription().to_string())
}

pub(super) fn make_shortcut(target: &Path, at: &Path) -> io::Result<()> {
    autoreleasepool(|_| {
        let target_url = file_url(target)?;
        // `writeBookmarkData` replaces an item that is there, and one another process made at
        // `at` since the caller looked must stay: the alias is written under a name of its own,
        // beside `at`, and moved into place without replacing anything.
        let temp = temp_beside(at);
        let alias_url = file_url(&temp)?;
        let data = target_url
            .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::SuitableForBookmarkFile,
                None,
                None,
            )
            .map_err(|error| io_error(&error))?;
        if let Err(error) = NSURL::writeBookmarkData_toURL_options_error(&data, &alias_url, 0) {
            // A half-written file is not left in the user's folder.
            let _ = std::fs::remove_file(&temp);
            return Err(io_error(&error));
        }
        move_into_place(&temp, at).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
    })
}

/// A name in the folder of `at` for an alias that is written there before it takes its place;
/// no two calls get the same one.
fn temp_beside(at: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    at.with_file_name(format!(".rf-link-{}-{number}", std::process::id()))
}

/// Renames `temp` to `at` and never replaces `at`: an item there fails with `AlreadyExists` and
/// stays as it is.
fn move_into_place(temp: &Path, at: &Path) -> io::Result<()> {
    let (Ok(from), Ok(to)) = (rf_core::VPath::local(temp), rf_core::VPath::local(at)) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an absolute path",
        ));
    };
    crate::rename_no_replace(&from, &to).map_err(|error| match error {
        rf_core::Error::Io { source, .. } => source,
        other => io::Error::other(other.to_string()),
    })
}

pub(super) fn read_shortcut(at: &Path) -> io::Result<Option<PathBuf>> {
    autoreleasepool(|_| {
        let url = file_url(at)?;
        // A file that holds no bookmark data is no alias.
        let Ok(data) = NSURL::bookmarkDataWithContentsOfURL_error(&url) else {
            return Ok(None);
        };
        let mut stale = Bool::NO;
        // SAFETY: `stale` is a place for a flag, valid for the call; no relative URL is used.
        let resolved = unsafe {
            NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                &data,
                NSURLBookmarkResolutionOptions::WithoutUI
                    | NSURLBookmarkResolutionOptions::WithoutMounting,
                None,
                &raw mut stale,
            )
        };
        // The target is gone, or on a volume that is not mounted: the alias points nowhere
        // this module can tell.
        let Ok(resolved) = resolved else {
            return Ok(None);
        };
        Ok(resolved.path().map(|path| PathBuf::from(path.to_string())))
    })
}

pub(super) fn make_junction(_target: &Path, _at: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "macOS makes no junctions",
    ))
}

pub(super) fn symlink_needs_privilege() -> bool {
    false
}
