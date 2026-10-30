#![allow(
    unsafe_code,
    reason = "the pasteboard types are extern statics of AppKit"
)]

//! The macOS general pasteboard, as Finder reads and writes it: one item for each file with its
//! file URL, the first one with the paths as text too.
//!
//! The file URLs are this module's own (`file_url_of`) and are read back by its own decoding
//! (`path_of_file_url`), byte for byte: `NSURL` hands a path back with its names decomposed
//! (NFD; `fileURLWithPath` and `path` go through the file system representation), so a name
//! written composed (NFC) would come back as other bytes. Only Finder's file reference URLs
//! (`file:///.file/id=…`), whose path names no item, are read through `NSURL`'s path URL, in the
//! form the system gives: APFS finds a file whatever the form of its name, and nothing here
//! normalizes a name another app wrote. Any other URL the decoding refuses is left out and
//! counted (`ClipboardFiles::skipped`, `pasted_url`): `NSURL` would read another computer's
//! `file://server/share/a` as the local `/share/a`, and an encoded `/` as part of another name.
//!
//! The pasteboard does not say whether files are to be moved: Finder moves what was copied with
//! ⌥⌘V. So the [`DropEffect`] of [`put_files`] is not kept, and [`get_files`] gives none.

use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeFileURL,
    NSPasteboardTypeString, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSString, NSURL};

use super::{ClipboardFiles, DropEffect, PastedUrl, file_url_of, pasted_url};

/// `public.file-url`.
fn file_url_type() -> &'static NSPasteboardType {
    // SAFETY: an immutable `NSString` constant of AppKit, set before any code of the app runs.
    unsafe { NSPasteboardTypeFileURL }
}

/// `public.utf8-plain-text`.
fn text_type() -> &'static NSPasteboardType {
    // SAFETY: as above.
    unsafe { NSPasteboardTypeString }
}

fn failed(what: &str) -> io::Error {
    io::Error::other(format!("the pasteboard refused {what}"))
}

/// A pasteboard change count as this module gives it.
fn count(change: isize) -> u64 {
    u64::try_from(change).unwrap_or(0)
}

/// The pasteboard item of the file at `path`, with its file URL; and `text` as text, if any.
fn item(path: &Path, text: Option<&str>) -> io::Result<Retained<NSPasteboardItem>> {
    let url = NSString::from_str(&file_url_of(path.as_os_str().as_bytes()));
    let item = NSPasteboardItem::new();
    if !item.setString_forType(&url, file_url_type()) {
        return Err(failed("a file URL"));
    }
    if let Some(text) = text
        && !item.setString_forType(&NSString::from_str(text), text_type())
    {
        return Err(failed("the text"));
    }
    Ok(item)
}

pub fn put_files(paths: &[PathBuf], _effect: DropEffect) -> io::Result<u64> {
    autoreleasepool(|_| {
        let lines: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let text = lines.join("\n");
        let items = paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                item(path, (i == 0).then_some(text.as_str())).map(ProtocolObject::from_retained)
            })
            .collect::<io::Result<Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>>>>()?;
        let board = NSPasteboard::generalPasteboard();
        let change = board.clearContents();
        if !board.writeObjects(&NSArray::from_retained_slice(&items)) {
            return Err(failed("the files"));
        }
        Ok(count(change))
    })
}

/// How many times [`get_files`] reads the pasteboard while another app writes to it.
const READ_TRIES: u32 = 3;

pub fn get_files() -> io::Result<Option<ClipboardFiles>> {
    autoreleasepool(|_| {
        let board = NSPasteboard::generalPasteboard();
        // The change count before and after the items: when they differ, another app wrote
        // while they were read, and they may be half its and half what was there.
        for _ in 0..READ_TRIES {
            let change = board.changeCount();
            let (paths, skipped) = file_paths(&board);
            if board.changeCount() != change {
                continue;
            }
            // File URLs that are all left out still answer, with no path: a paste says why it
            // pasted nothing.
            return Ok(
                (!paths.is_empty() || skipped > 0).then_some(ClipboardFiles {
                    paths,
                    effect: None,
                    change: count(change),
                    skipped,
                }),
            );
        }
        Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            "the pasteboard kept changing while it was read",
        ))
    })
}

/// The paths of the file URLs of the pasteboard's items, and how many of those URLs name no
/// path that can be read safely (`PastedUrl::Skipped`, and a file reference URL whose item the
/// system does not find).
fn file_paths(board: &NSPasteboard) -> (Vec<PathBuf>, usize) {
    let items = board
        .pasteboardItems()
        .map(|items| items.to_vec())
        .unwrap_or_default();
    let mut paths = Vec::with_capacity(items.len());
    let mut skipped = 0;
    for item in &items {
        let Some(text) = item.stringForType(file_url_type()) else {
            continue;
        };
        let path = match pasted_url(&text.to_string()) {
            // A path URL as it was written, byte for byte.
            PastedUrl::Path(bytes) => Some(PathBuf::from(OsString::from_vec(bytes))),
            // A file reference URL (Finder's) becomes the path URL of its item.
            PastedUrl::Reference => reference_path(&text),
            PastedUrl::Skipped => None,
        };
        match path {
            Some(path) => paths.push(path),
            None => skipped += 1,
        }
    }
    (paths, skipped)
}

/// The path of the item the file reference URL `text` refers to; `None` when the system does
/// not find it.
fn reference_path(text: &NSString) -> Option<PathBuf> {
    let url = NSURL::URLWithString(text)?;
    let url = url.filePathURL()?;
    url.isFileURL()
        .then(|| url.path())
        .flatten()
        .map(|path| PathBuf::from(path.to_string()))
}

pub fn has_files() -> bool {
    autoreleasepool(|_| {
        NSPasteboard::generalPasteboard()
            .canReadItemWithDataConformingToTypes(&NSArray::from_slice(&[file_url_type()]))
    })
}

pub fn put_text(text: &str) -> io::Result<u64> {
    autoreleasepool(|_| {
        let board = NSPasteboard::generalPasteboard();
        let change = board.clearContents();
        if !board.setString_forType(&NSString::from_str(text), text_type()) {
            return Err(failed("the text"));
        }
        Ok(count(change))
    })
}

pub fn get_text() -> io::Result<Option<String>> {
    autoreleasepool(|_| {
        let board = NSPasteboard::generalPasteboard();
        Ok(board
            .stringForType(text_type())
            .map(|text| text.to_string()))
    })
}

pub fn change_count() -> Option<u64> {
    autoreleasepool(|_| u64::try_from(NSPasteboard::generalPasteboard().changeCount()).ok())
}

pub fn clear_if_unchanged(change: u64) -> io::Result<bool> {
    autoreleasepool(|_| {
        let board = NSPasteboard::generalPasteboard();
        if count(board.changeCount()) != change {
            return Ok(false);
        }
        board.clearContents();
        Ok(true)
    })
}

pub fn universal_path(_path: &Path) -> Option<PathBuf> {
    None
}
