#![allow(
    unsafe_code,
    reason = "the Win32 clipboard, its global memory and WNet"
)]

//! The Windows clipboard, through the Win32 calls, as File Explorer reads and writes it: files
//! as `CF_HDROP` with `Preferred DropEffect` and the paths as `CF_UNICODETEXT`.
//!
//! The clipboard is opened without an owner window (as the clipboard crates do), on the calling
//! thread, and closed on that thread before the call returns; another app that holds it open
//! is waited for up to [`OPEN_TRIES`] times [`OPEN_PAUSE`]. A block of global memory goes to
//! the system with `SetClipboardData`: from a call that worked on, the system owns it and frees
//! it; until then [`Global`] frees it.

use std::ffi::OsString;
use std::io;
use std::marker::PhantomData;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_CONNECTION_UNAVAIL, ERROR_MORE_DATA, GlobalFree, HANDLE, HGLOBAL, NO_ERROR,
};
use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::{PCWSTR, PWSTR, w};

use super::{
    CF_HDROP, CF_UNICODETEXT, ClipboardFiles, DropEffect, DropList, drop_effect_bytes, drop_files,
    path_lines, read_drop_effect, read_drop_files, read_unicode_text, unc_path, unicode_text,
};
use crate::win_util::to_io;

/// How many times the clipboard is tried while another app holds it open, and the pause
/// between two tries: about 100 ms in all.
const OPEN_TRIES: u32 = 10;
const OPEN_PAUSE: Duration = Duration::from_millis(10);

/// The clipboard, open on this thread until it is dropped. It stays on the thread that opened
/// it (not `Send`), since only that thread may close it.
struct Opened(PhantomData<*const ()>);

/// Opens the clipboard, trying again while another app holds it.
fn open() -> io::Result<Opened> {
    for attempt in 1..=OPEN_TRIES {
        // SAFETY: no owner window is passed; the clipboard is closed on this thread by the
        // guard's drop, and no other call of this module opens it while the guard lives.
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(Opened(PhantomData));
        }
        if attempt < OPEN_TRIES {
            std::thread::sleep(OPEN_PAUSE);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::ResourceBusy,
        "another app holds the clipboard open",
    ))
}

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: this thread opened the clipboard (`open`) and has not closed it.
        let _ = unsafe { CloseClipboard() };
    }
}

/// The clipboard's change count; 0 when this process may not see the clipboard.
fn sequence() -> u32 {
    // SAFETY: takes nothing and only reads the window station's count.
    unsafe { GetClipboardSequenceNumber() }
}

/// The number of the registered format `Preferred DropEffect`.
fn preferred_drop_effect() -> io::Result<u32> {
    // SAFETY: the name is a NUL-terminated literal that lives as long as the program.
    let format = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    if format == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(format)
}

/// A block of movable global memory that is ours until the clipboard takes it.
struct Global(HGLOBAL);

impl Global {
    /// A new block holding `bytes`.
    fn with(bytes: &[u8]) -> io::Result<Global> {
        // SAFETY: a plain allocation of `bytes.len()` bytes (at least one: every buffer here
        // ends with a NUL), which `Global` frees unless it is handed over.
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.map_err(|e| to_io(&e))?;
        let global = Global(handle);
        // SAFETY: `handle` is a movable block that nobody else has locked.
        let target = unsafe { GlobalLock(handle) };
        if target.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the locked block is at least `bytes.len()` bytes long, and it is new, so it
        // does not overlap `bytes`.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast::<u8>(), bytes.len()) };
        // SAFETY: this thread locked it once above. The call says "failed" when the last lock
        // goes, which is what is wanted here.
        let _ = unsafe { GlobalUnlock(handle) };
        Ok(global)
    }

    /// Hands the block to the clipboard as `format`. From a call that worked on the system owns
    /// it; a call that failed leaves it ours, and the drop frees it.
    fn hand_over(self, format: u32) -> io::Result<()> {
        // SAFETY: this thread holds the clipboard open (`put` opened it and emptied it; the
        // owner window is NULL, as `OpenClipboard(None)` leaves it), which is what
        // `SetClipboardData` needs; the block is an unlocked movable one that nothing else
        // refers to.
        match unsafe { SetClipboardData(format, Some(HANDLE(self.0.0))) } {
            Ok(_) => {
                std::mem::forget(self);
                Ok(())
            }
            Err(error) => Err(to_io(&error)),
        }
    }
}

impl Drop for Global {
    fn drop(&mut self) {
        // SAFETY: the block is still ours (`hand_over` forgets it once the system has it), and
        // it is not locked. `GlobalFree` gives NULL when it works, which the wrapper reports as
        // an error; there is nothing to do either way.
        let _ = unsafe { GlobalFree(Some(self.0)) };
    }
}

/// Puts the blocks `blocks` (each with its format) on the clipboard in the place of what it
/// held, and gives the change count after it. Every block is made before the clipboard is
/// opened, so it is held as briefly as can be; when one cannot be handed over, the clipboard is
/// emptied, so that no part of the content stays without the rest.
fn put(blocks: Vec<(u32, Vec<u8>)>) -> io::Result<u64> {
    let blocks: Vec<(u32, Global)> = blocks
        .into_iter()
        .map(|(format, bytes)| Global::with(&bytes).map(|global| (format, global)))
        .collect::<io::Result<_>>()?;
    {
        let _open = open()?;
        // SAFETY: the clipboard is open on this thread.
        unsafe { EmptyClipboard() }.map_err(|e| to_io(&e))?;
        for (format, global) in blocks {
            if let Err(error) = global.hand_over(format) {
                // SAFETY: as above; what was handed over already goes with the rest.
                let _ = unsafe { EmptyClipboard() };
                return Err(error);
            }
        }
    }
    // After the close: the count is the one another app sees from now on.
    Ok(u64::from(sequence()))
}

/// The bytes of the clipboard's block of `format`; `None` when the clipboard has none. The
/// clipboard must be open on this thread (`_open`); the block stays the clipboard's.
fn read(_open: &Opened, format: u32) -> io::Result<Option<Vec<u8>>> {
    // SAFETY: the clipboard is open on this thread.
    if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
        return Ok(None);
    }
    // SAFETY: as above; the handle stays valid while the clipboard is open and nobody empties
    // it, which only the owner of an open clipboard can do.
    let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
        return Ok(None);
    };
    let global = HGLOBAL(handle.0);
    // SAFETY: a clipboard block of a format whose data is global memory.
    let size = unsafe { GlobalSize(global) };
    // SAFETY: as above; locked once here and unlocked below.
    let source = unsafe { GlobalLock(global) };
    if source.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the locked block is `size` bytes long and stays locked while it is copied.
    let bytes = unsafe { std::slice::from_raw_parts(source.cast::<u8>(), size) }.to_vec();
    // SAFETY: this thread locked it once above.
    let _ = unsafe { GlobalUnlock(global) };
    Ok(Some(bytes))
}

/// The paths of a narrow (ANSI) `CF_HDROP` list, which only the system can read. The clipboard
/// must be open on this thread (`_open`).
fn narrow_paths(_open: &Opened) -> Vec<Vec<u16>> {
    // SAFETY: the clipboard is open on this thread and has `CF_HDROP` (the caller read it).
    let Ok(handle) = (unsafe { GetClipboardData(CF_HDROP) }) else {
        return Vec::new();
    };
    let hdrop = HDROP(handle.0);
    // SAFETY: `hdrop` is the clipboard's `CF_HDROP` block; 0xFFFF_FFFF asks for the count.
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    (0..count)
        .filter_map(|index| {
            // SAFETY: as above; without a buffer the call gives the length of path `index`.
            let length = unsafe { DragQueryFileW(hdrop, index, None) };
            let mut buffer = vec![0u16; usize::try_from(length).ok()? + 1];
            // SAFETY: as above; the buffer has room for the path and its NUL.
            let written = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) };
            buffer.truncate(usize::try_from(written).ok()?);
            (!buffer.is_empty()).then_some(buffer)
        })
        .collect()
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().collect()
}

pub fn put_files(paths: &[PathBuf], effect: DropEffect) -> io::Result<u64> {
    let units: Vec<Vec<u16>> = paths.iter().map(|path| wide(path)).collect();
    let list = drop_files(&units).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a path the clipboard cannot hold",
        )
    })?;
    put(vec![
        (CF_HDROP, list),
        (preferred_drop_effect()?, drop_effect_bytes(effect).to_vec()),
        (CF_UNICODETEXT, path_lines(&units)),
    ])
}

pub fn get_files() -> io::Result<Option<ClipboardFiles>> {
    let effect_format = preferred_drop_effect()?;
    let open = open()?;
    let change = u64::from(sequence());
    let Some(list) = read(&open, CF_HDROP)? else {
        return Ok(None);
    };
    let units = match read_drop_files(&list) {
        Some(DropList::Wide(units)) => units,
        Some(DropList::Narrow) => narrow_paths(&open),
        None => Vec::new(),
    };
    if units.is_empty() {
        return Ok(None);
    }
    let effect =
        read(&open, effect_format)?.map_or(DropEffect::Copy, |bytes| read_drop_effect(&bytes));
    let paths = units
        .iter()
        .map(|units| PathBuf::from(OsString::from_wide(units)))
        .collect();
    Ok(Some(ClipboardFiles {
        paths,
        effect: Some(effect),
        change,
        skipped: 0,
    }))
}

pub fn has_files() -> bool {
    // SAFETY: takes a format number and only asks the window station; unlike reading the data,
    // it needs no hold of the clipboard (`open`).
    unsafe { IsClipboardFormatAvailable(CF_HDROP) }.is_ok()
}

pub fn put_text(text: &str) -> io::Result<u64> {
    put(vec![(CF_UNICODETEXT, unicode_text(text.encode_utf16()))])
}

pub fn get_text() -> io::Result<Option<String>> {
    let open = open()?;
    Ok(read(&open, CF_UNICODETEXT)?.map(|bytes| read_unicode_text(&bytes)))
}

pub fn change_count() -> Option<u64> {
    match sequence() {
        0 => None,
        count => Some(u64::from(count)),
    }
}

pub fn clear_if_unchanged(change: u64) -> io::Result<bool> {
    let _open = open()?;
    // Looked at while the clipboard is held: nobody can put anything on it in between.
    if u64::from(sequence()) != change {
        return Ok(false);
    }
    // SAFETY: the clipboard is open on this thread.
    unsafe { EmptyClipboard() }.map_err(|e| to_io(&e))?;
    Ok(true)
}

pub fn universal_path(path: &Path) -> Option<PathBuf> {
    let local = wide(path);
    let [letter, colon, ..] = local.as_slice() else {
        return None;
    };
    // Only a drive letter can be mapped to a share: anything else is not asked about.
    let is_letter = u8::try_from(*letter).is_ok_and(|letter| letter.is_ascii_alphabetic());
    if !is_letter || *colon != u16::from(b':') {
        return None;
    }
    let device = [*letter, *colon, 0];
    let mut size: u32 = 512;
    loop {
        let mut buffer = vec![0u16; usize::try_from(size).ok()?];
        let mut length = size;
        // SAFETY: `device` is a NUL-terminated drive name (`Z:`), `buffer` has room for
        // `length` units, and `length` is a live `u32` the call writes the size it needs to.
        let code = unsafe {
            WNetGetConnectionW(
                PCWSTR(device.as_ptr()),
                Some(PWSTR(buffer.as_mut_ptr())),
                &raw mut length,
            )
        };
        // A remembered connection that is not up now still names its share.
        if code == NO_ERROR || code == ERROR_CONNECTION_UNAVAIL {
            let end = buffer.iter().position(|&unit| unit == 0)?;
            let unc = unc_path(&local, &buffer[..end])?;
            return Some(PathBuf::from(OsString::from_wide(&unc)));
        }
        if code != ERROR_MORE_DATA || length <= size {
            return None;
        }
        size = length;
    }
}
