#![allow(unsafe_code, reason = "ReadDirectoryChangesW FFI")]

//! Directory watching on Windows: a thread that calls `ReadDirectoryChangesW` in a loop.
//!
//! The call is synchronous and blocks until something changes. Dropping the watch cancels it
//! from outside with `CancelIoEx`.

use std::ffi::{OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{ERROR_NOTIFY_ENUM_DIR, ERROR_OPERATION_ABORTED, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::CancelIoEx;

use super::records::{pair_renames, parse_notify_records};
use super::{DirEvent, DirEventHandler};

/// Size of the buffer that `ReadDirectoryChangesW` fills: 64 KiB, the most that works on a
/// network share. The buffer is `u32` elements because the records need 4-byte alignment.
const BUFFER_BYTES: u32 = 65_536;
const BUFFER_WORDS: usize = 16_384;

/// Names being created, renamed or deleted, and attributes, size, write time and creation time
/// changing. Reading a file (`LAST_ACCESS`) and security descriptors are not changes to show.
const FILTER: FILE_NOTIFY_CHANGE = FILE_NOTIFY_CHANGE(
    FILE_NOTIFY_CHANGE_FILE_NAME.0
        | FILE_NOTIFY_CHANGE_DIR_NAME.0
        | FILE_NOTIFY_CHANGE_ATTRIBUTES.0
        | FILE_NOTIFY_CHANGE_SIZE.0
        | FILE_NOTIFY_CHANGE_LAST_WRITE.0
        | FILE_NOTIFY_CHANGE_CREATION.0,
);

/// The watched folder and the thread that reads its changes.
#[derive(Debug)]
pub(super) struct Watcher {
    /// The folder, opened for listing. Shared with the thread, so that the handle outlives
    /// every call the thread makes with it.
    folder: Arc<File>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Opens `dir` and starts the thread that reports its changes to `handler`.
pub(super) fn start(dir: &Path, handler: DirEventHandler) -> io::Result<Watcher> {
    // Backup semantics let a folder open; full sharing keeps other programs from being blocked
    // by the watch, deleting the folder included.
    let folder = Arc::new(
        OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(dir)?,
    );
    let stop = Arc::new(AtomicBool::new(false));
    let thread = std::thread::Builder::new().name("rf-watch".into()).spawn({
        let (folder, stop) = (Arc::clone(&folder), Arc::clone(&stop));
        move || run(&folder, &stop, handler)
    })?;
    Ok(Watcher {
        folder,
        stop,
        thread: Some(thread),
    })
}

impl Drop for Watcher {
    /// Blocks until the watch thread is gone, at least one 10 ms pause; see `DirWatch`.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let handle = HANDLE(self.folder.as_raw_handle());
        // One cancel is not enough: if the thread has looked at `stop` but not yet entered
        // `ReadDirectoryChangesW`, nothing is pending to cancel, and the call then blocks for
        // good. So cancel until the thread is gone.
        while !thread.is_finished() {
            // SAFETY: `handle` belongs to `self.folder`, which is open until `self` is gone.
            // `CancelIoEx` without an `OVERLAPPED` cancels every pending call on the handle. It
            // fails when none is pending, which is fine, so the result is ignored.
            let _ = unsafe { CancelIoEx(handle, None) };
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = thread.join();
    }
}

/// The body of the watch thread: read changes and hand them over until told to stop, or until
/// the folder or the call is gone. Once `stop` is set, `handler` is not called again.
fn run(folder: &File, stop: &AtomicBool, mut handler: DirEventHandler) {
    let handle = HANDLE(folder.as_raw_handle());
    let mut buffer = vec![0_u32; BUFFER_WORDS];
    while !stop.load(Ordering::SeqCst) {
        let mut returned = 0_u32;
        // SAFETY: `handle` belongs to `folder`, which is open for the call. `buffer` holds
        // `BUFFER_BYTES` writable bytes and `returned` is writable, both for the whole call:
        // with no `OVERLAPPED` and no completion routine the call is synchronous and the
        // kernel has finished with both when it returns.
        let result = unsafe {
            ReadDirectoryChangesW(
                handle,
                buffer.as_mut_ptr().cast::<c_void>(),
                BUFFER_BYTES,
                false,
                FILTER,
                Some(&raw mut returned),
                None,
                None,
            )
        };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match result {
            // A success with no data means that more changed than the buffer holds.
            Ok(()) if returned == 0 => handler(DirEvent::Overflow),
            Ok(()) => {
                for event in events(&buffer, returned) {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    handler(event);
                }
            }
            Err(e) if e.code() == ERROR_OPERATION_ABORTED.to_hresult() => return,
            Err(e) if e.code() == ERROR_NOTIFY_ENUM_DIR.to_hresult() => {
                handler(DirEvent::Overflow);
            }
            // Deleting the watched folder permanently fails the call with `ERROR_ACCESS_DENIED`;
            // any other failure means that the watch is dead as well, though the folder may
            // still be there (a network share that dropped, a file system without change
            // notifications). `Ended` does not tell the two apart; the receiver looks. A move or
            // rename of the folder does not fail the call: the handle follows the folder.
            Err(_) => {
                handler(DirEvent::Ended);
                return;
            }
        }
    }
}

/// The events in the first `returned` bytes of `buffer`.
fn events(buffer: &[u32], returned: u32) -> Vec<DirEvent> {
    let filled = usize::try_from(returned).unwrap_or(usize::MAX);
    let bytes: Vec<u8> = buffer
        .iter()
        .flat_map(|word| word.to_ne_bytes())
        .take(filled)
        .collect();
    pair_renames(
        parse_notify_records(&bytes)
            .into_iter()
            .map(|(action, name)| (action, OsString::from_wide(&name)))
            .collect(),
    )
}
