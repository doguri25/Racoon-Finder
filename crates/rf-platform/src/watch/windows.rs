#![allow(unsafe_code, reason = "ReadDirectoryChangesW FFI")]

//! Directory watching on Windows: a thread that calls `ReadDirectoryChangesW` in a loop.
//!
//! Each call is overlapped: it returns once the kernel watches the folder, and the thread then
//! waits for it to complete. [`start`] returns only once the first call is in place, so a
//! change made after it returns is never missed: from that first call on, the kernel keeps what
//! changes between two calls on the folder's handle. Dropping the watch cancels the pending call
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
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_NOTIFY_ENUM_DIR, ERROR_OPERATION_ABORTED, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::CreateEventW;
use windows::core::PCWSTR;

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

/// Opens `dir` and starts the thread that reports its changes to `handler`. Returns once the
/// thread's first `ReadDirectoryChangesW` watches the folder; a first call that fails (a file
/// system without change notifications) fails the start with its error.
pub(super) fn start(dir: &Path, handler: DirEventHandler) -> io::Result<Watcher> {
    // Backup semantics let a folder open; full sharing keeps other programs from being blocked
    // by the watch, deleting the folder included. Overlapped calls return once they watch.
    let folder = Arc::new(
        OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OVERLAPPED.0)
            .open(dir)?,
    );
    let stop = Arc::new(AtomicBool::new(false));
    let (armed, first) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new().name("rf-watch".into()).spawn({
        let (folder, stop) = (Arc::clone(&folder), Arc::clone(&stop));
        move || run(&folder, &stop, handler, armed)
    })?;
    let watcher = Watcher {
        folder,
        stop,
        thread: Some(thread),
    };
    // A thread that ended without a word (a panic) watches nothing; dropping the watcher then
    // joins it, as it does a thread whose first call failed.
    let first = first
        .recv()
        .unwrap_or_else(|_| Err(io::Error::other("the watch thread ended before it watched")));
    first.map(|()| watcher)
}

impl Drop for Watcher {
    /// Blocks until the watch thread is gone: one 10 ms pause or more while it still runs; see
    /// `DirWatch`.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let handle = HANDLE(self.folder.as_raw_handle());
        // One cancel is not enough: if the thread has looked at `stop` but not yet put its next
        // `ReadDirectoryChangesW` in place, nothing is pending to cancel, and the thread then
        // waits in `GetOverlappedResult` for good. So cancel until the thread is gone: a cancel
        // completes the pending call, which ends that wait.
        while !thread.is_finished() {
            // SAFETY: `handle` belongs to `self.folder`, which is open until `self` is gone.
            // `CancelIoEx` without an `OVERLAPPED` cancels every pending call on the handle (the
            // thread's own `OVERLAPPED` stays alive until its wait has seen the call end). It
            // fails when none is pending, which is fine, so the result is ignored.
            let _ = unsafe { CancelIoEx(handle, None) };
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = thread.join();
    }
}

/// The body of the watch thread: puts a call in place, waits for it to complete and hands its
/// changes over, until told to stop, or until the folder or the call is gone. How the first
/// call went is sent to `armed` as soon as it is in place (or failed); a first call that fails
/// ends the thread with nothing else told. Once `stop` is set, `handler` is not called again.
fn run(
    folder: &File,
    stop: &AtomicBool,
    mut handler: DirEventHandler,
    armed: SyncSender<io::Result<()>>,
) {
    let handle = HANDLE(folder.as_raw_handle());
    let mut buffer = vec![0_u32; BUFFER_WORDS];
    let done = match Event::new() {
        Ok(done) => done,
        Err(error) => {
            let _ = armed.send(Err(error));
            return;
        }
    };
    let mut armed = Some(armed);
    while !stop.load(Ordering::SeqCst) {
        let mut overlapped = OVERLAPPED {
            hEvent: done.0,
            ..OVERLAPPED::default()
        };
        // SAFETY: `handle` belongs to `folder`, which is open for the call and was opened for
        // overlapped calls. `buffer` holds `BUFFER_BYTES` writable bytes and `overlapped` is a
        // writable `OVERLAPPED` whose event `done` is open; the kernel may use all three until
        // the call completes, and nothing touches, moves or frees them before
        // `GetOverlappedResult` below has waited for that. No completion routine is given.
        let issued = unsafe {
            ReadDirectoryChangesW(
                handle,
                buffer.as_mut_ptr().cast::<c_void>(),
                BUFFER_BYTES,
                false,
                FILTER,
                None,
                Some(&raw mut overlapped),
                None,
            )
        };
        if let Some(armed) = armed.take() {
            let first = issued.clone().map_err(io::Error::from);
            let _ = armed.send(first);
            if issued.is_err() {
                return;
            }
        }
        if issued.is_err() {
            // The folder cannot be watched again: the watch is dead.
            if !stop.load(Ordering::SeqCst) {
                handler(DirEvent::Ended);
            }
            return;
        }
        let mut returned = 0_u32;
        // SAFETY: `overlapped` is the one of the call above, which is in place, and `returned`
        // is writable. With `bwait` the function returns only once the call has completed
        // (`CancelIoEx` completes it too), so the kernel is done with `buffer` and
        // `overlapped` when it returns.
        let result =
            unsafe { GetOverlappedResult(handle, &raw const overlapped, &raw mut returned, true) };
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
            // still be there (a network share that dropped). `Ended` does not tell the two
            // apart; the receiver looks. A move or rename of the folder does not fail the call:
            // the handle follows the folder.
            Err(_) => {
                handler(DirEvent::Ended);
                return;
            }
        }
    }
}

/// A manual-reset event that a pending call signals when it completes, closed when dropped.
struct Event(HANDLE);

impl Event {
    fn new() -> io::Result<Event> {
        // SAFETY: no security attributes and no name: a new unnamed event of this process,
        // manual-reset and not signalled. The call resets it each time a call starts.
        let handle = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?;
        Ok(Event(handle))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle is this event's own, open until now, and no call that signals it
        // is pending: `run` drops it only after the last call has completed.
        let _ = unsafe { CloseHandle(self.0) };
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
