#![allow(
    unsafe_code,
    reason = "the pasteboard types are extern statics of AppKit"
)]

//! The macOS general pasteboard, as Finder reads and writes it: one item for each file with its
//! file URL, the first one with the paths as text too. Finder writes file reference URLs
//! (`file:///.file/id=…`), which are read through their path URLs.
//!
//! The pasteboard does not say whether files are to be moved: Finder moves what was copied with
//! ⌥⌘V. So the [`DropEffect`] of [`put_files`] is not kept, and [`get_files`] gives none.

use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeFileURL,
    NSPasteboardTypeString, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSString, NSURL};

use super::{ClipboardFiles, DropEffect};

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
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a path that is not UTF-8"))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path))
        .absoluteString()
        .ok_or_else(|| failed("a file URL"))?;
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
            let paths = file_paths(&board);
            if board.changeCount() != change {
                continue;
            }
            return Ok((!paths.is_empty()).then_some(ClipboardFiles {
                paths,
                effect: None,
                change: count(change),
            }));
        }
        Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            "the pasteboard kept changing while it was read",
        ))
    })
}

/// The paths of the file URLs of the pasteboard's items.
fn file_paths(board: &NSPasteboard) -> Vec<PathBuf> {
    let items = board
        .pasteboardItems()
        .map(|items| items.to_vec())
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|item| {
            let text = item.stringForType(file_url_type())?;
            // A file reference URL (Finder's) becomes the path URL of its item.
            let url = NSURL::URLWithString(&text)?;
            let url = url.filePathURL()?;
            url.isFileURL()
                .then(|| url.path())
                .flatten()
                .map(|path| PathBuf::from(path.to_string()))
        })
        .collect()
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
