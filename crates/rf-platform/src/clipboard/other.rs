//! Systems other than Windows and macOS (Linux, for development): files on the clipboard stay
//! in this process, which finds them again; other apps do not see them. Text is unsupported,
//! as no system clipboard is reached without a crate of its own.

#![allow(
    clippy::unnecessary_wraps,
    reason = "the signatures of every system's module, which can fail elsewhere"
)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::{ClipboardFiles, DropEffect};

/// What this process put on its clipboard, and the count of its changes.
#[derive(Debug)]
struct Store {
    files: Option<(Vec<PathBuf>, DropEffect)>,
    change: u64,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    files: None,
    change: 1,
});

/// The store; one that a panic left behind holds whole values all the same.
fn store() -> MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn no_text() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "no text clipboard on this system",
    )
}

pub fn put_files(paths: &[PathBuf], effect: DropEffect) -> io::Result<u64> {
    let mut store = store();
    store.files = Some((paths.to_vec(), effect));
    store.change += 1;
    Ok(store.change)
}

pub fn get_files() -> io::Result<Option<ClipboardFiles>> {
    let store = store();
    Ok(store.files.as_ref().map(|(paths, effect)| ClipboardFiles {
        paths: paths.clone(),
        effect: Some(*effect),
        change: store.change,
    }))
}

pub fn put_text(_text: &str) -> io::Result<u64> {
    Err(no_text())
}

pub fn get_text() -> io::Result<Option<String>> {
    Err(no_text())
}

pub fn change_count() -> Option<u64> {
    Some(store().change)
}

pub fn clear_if_unchanged(change: u64) -> io::Result<bool> {
    let mut store = store();
    if store.change != change {
        return Ok(false);
    }
    store.files = None;
    store.change += 1;
    Ok(true)
}

pub fn universal_path(_path: &Path) -> Option<PathBuf> {
    None
}
