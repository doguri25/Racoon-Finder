#![allow(unsafe_code, reason = "SetFileAttributesW and FindFirstStreamW FFI")]

//! Times, attributes and alternate data streams on Windows.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::{File, FileTimes, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{FileTimesExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rf_core::{Attributes, Error, Result, VPath};
use windows::Win32::Foundation::{ERROR_HANDLE_EOF, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_PINNED,
    FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, FILE_ATTRIBUTE_TEMPORARY,
    FILE_ATTRIBUTE_UNPINNED, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_ATTRIBUTES, FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
    SetFileAttributesW, WIN32_FIND_STREAM_DATA,
};
use windows::core::PCWSTR;

use super::keep_first;
use crate::win_util::{to_io, wide};

pub fn set_times(
    local: &Path,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
    created: Option<SystemTime>,
) -> io::Result<()> {
    // Attribute-write access with full sharing works on a read-only or open file; backup
    // semantics let a folder open; the reparse flag keeps a final link from being followed.
    let file = OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(local)?;
    let mut times = FileTimes::new();
    if let Some(modified) = modified {
        times = times.set_modified(modified);
    }
    if let Some(accessed) = accessed {
        times = times.set_accessed(accessed);
    }
    if let Some(created) = created {
        times = times.set_created(created);
    }
    file.set_times(times)
}

/// The attribute bits `set_attributes` sets exactly as asked.
const EXACT: [(Attributes, FILE_FLAGS_AND_ATTRIBUTES); 4] = [
    (Attributes::READONLY, FILE_ATTRIBUTE_READONLY),
    (Attributes::HIDDEN, FILE_ATTRIBUTE_HIDDEN),
    (Attributes::SYSTEM, FILE_ATTRIBUTE_SYSTEM),
    (Attributes::ARCHIVE, FILE_ATTRIBUTE_ARCHIVE),
];

/// Bits `SetFileAttributesW` can set that `set_attributes` passes on as they are, so that they
/// are not cleared.
const KEPT: FILE_FLAGS_AND_ATTRIBUTES = FILE_FLAGS_AND_ATTRIBUTES(
    FILE_ATTRIBUTE_TEMPORARY.0
        | FILE_ATTRIBUTE_OFFLINE.0
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED.0
        | FILE_ATTRIBUTE_PINNED.0
        | FILE_ATTRIBUTE_UNPINNED.0,
);

pub fn set_attributes(local: &Path, attributes: Attributes) -> io::Result<()> {
    let current = std::fs::symlink_metadata(local)?.file_attributes();
    let bits = EXACT
        .iter()
        .filter(|(attribute, _)| attributes.contains(*attribute))
        .fold(current & KEPT.0, |bits, (_, bit)| bits | bit.0);
    // No bit at all must be said as `FILE_ATTRIBUTE_NORMAL`.
    let bits = if bits == 0 {
        FILE_ATTRIBUTE_NORMAL.0
    } else {
        bits
    };
    let name = wide(local.as_os_str());
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(bits)) }
        .map_err(|e| to_io(&e))
}

pub fn copy_extras(from: &VPath, to: &VPath, source: &Path, target: &Path) -> Result<()> {
    let streams = stream_names(source).map_err(|e| Error::io("copy_extras", from.clone(), e))?;
    let mut first = None;
    for stream in streams {
        if let Err(e) = copy_stream(from, to, source, target, &stream) {
            keep_first(&mut first, e);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Copies the alternate data stream `stream` (`:name:$DATA`) of `source` to `target`, replacing
/// a stream of that name there.
fn copy_stream(
    from: &VPath,
    to: &VPath,
    source: &Path,
    target: &Path,
    stream: &OsStr,
) -> Result<()> {
    let mut reader = File::open(with_stream(source, stream))
        .map_err(|e| Error::io("copy_extras", from.clone(), e))?;
    let mut writer = File::create(with_stream(target, stream))
        .map_err(|e| Error::io("copy_extras", to.clone(), e))?;
    io::copy(&mut reader, &mut writer).map_err(|e| Error::io("copy_extras", to.clone(), e))?;
    Ok(())
}

/// `local` followed by a stream name such as `:Zone.Identifier:$DATA`.
fn with_stream(local: &Path, stream: &OsStr) -> PathBuf {
    let mut path = local.as_os_str().to_owned();
    path.push(stream);
    PathBuf::from(path)
}

/// Closes a stream search when it is dropped.
struct StreamSearch(HANDLE);

impl Drop for StreamSearch {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `FindFirstStreamW` and is closed only here.
        // A failed close leaves nothing to do.
        let _ = unsafe { FindClose(self.0) };
    }
}

/// The names (`:name:$DATA`) of the alternate data streams of `local`, without the main
/// `::$DATA` stream.
///
/// If the search cannot even start, the file system knows no streams (FAT, exFAT), or the item
/// has none at all (a folder), and the answer is an empty list.
fn stream_names(local: &Path) -> io::Result<Vec<OsString>> {
    let name = wide(local.as_os_str());
    let mut data = WIN32_FIND_STREAM_DATA::default();
    // SAFETY: `name` is NUL-terminated and outlives the call, and `data` is a writable
    // `WIN32_FIND_STREAM_DATA`, which is what `FindStreamInfoStandard` fills in.
    let first = unsafe {
        FindFirstStreamW(
            PCWSTR(name.as_ptr()),
            FindStreamInfoStandard,
            (&raw mut data).cast::<c_void>(),
            None,
        )
    };
    let Ok(handle) = first else {
        return Ok(Vec::new());
    };
    let search = StreamSearch(handle);
    let mut names = Vec::new();
    loop {
        let len = data
            .cStreamName
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(data.cStreamName.len());
        let stream = OsString::from_wide(&data.cStreamName[..len]);
        if stream != "::$DATA" {
            names.push(stream);
        }
        // SAFETY: `search` holds an open search handle, and `data` is a writable
        // `WIN32_FIND_STREAM_DATA`.
        match unsafe { FindNextStreamW(search.0, (&raw mut data).cast::<c_void>()) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_HANDLE_EOF.into() => return Ok(names),
            Err(e) => return Err(to_io(&e)),
        }
    }
}
