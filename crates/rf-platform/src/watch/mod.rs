//! Raw change notifications for one folder, the layer under live folder refresh.
//!
//! The OS tells which names in a folder changed, not how, and it may drop or merge
//! notifications. So an event only says "look at this name again" (the receiver calls `stat`
//! to see what is there now), or "read the whole folder again" after an overflow.
//!
//! Windows uses `ReadDirectoryChangesW` on a thread of its own; macOS and Linux use the
//! `notify` crate (`FSEvents`, inotify).

use std::ffi::OsString;
use std::io;
#[cfg(not(windows))]
use std::path::Path;

use rf_core::{Error, Result, VPath};

#[cfg(not(windows))]
mod notify_backend;
mod records;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
use self::windows::{Watcher as Backend, start};

/// What happened in a watched folder. Every name is one name directly inside the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirEvent {
    /// The entry `name` appeared, changed or disappeared. The event does not say which; the
    /// receiver looks at the entry with `stat`.
    Changed(OsString),
    /// The entry `from` was renamed to `to`. Only sent when the OS pairs the two names; without
    /// a pair, each name is a `Changed`.
    Renamed { from: OsString, to: OsString },
    /// Notifications were lost. The receiver reads the folder again.
    Overflow,
    /// The folder itself is gone, or the watch cannot go on. No event follows.
    Ended,
}

/// Receives the events of one watch, on a thread of the backend. It must not block for long, or
/// the OS may drop notifications (`DirEvent::Overflow`).
pub type DirEventHandler = Box<dyn FnMut(DirEvent) + Send>;

/// A running watch. Dropping it stops the watch.
///
/// On Windows the handler is not called again once the drop has returned. The `notify` backend
/// (inotify, `FSEvents`) can still call it once more right after the drop, so a caller that must
/// not see an event after a drop has to ignore it on its own side.
///
/// Never drop a watch from inside its own handler: dropping can wait for the thread that runs
/// the handler, which would then be waiting for itself.
///
/// After `DirEvent::Ended` the folder handle can stay open on Windows until the watch is
/// dropped, which keeps the folder from disappearing; the receiver drops the watch right away.
#[derive(Debug)]
pub struct DirWatch {
    _backend: Backend,
}

#[cfg(not(windows))]
type Backend = notify::RecommendedWatcher;

/// Watches the folder `dir` for changes to its direct children; nothing inside a subfolder is
/// reported. `handler` is called on a thread of the backend, so it needs to be quick.
///
/// A folder that does not exist gives `ErrorKind::NotFound` and something that is not a folder
/// gives `ErrorKind::NotADirectory`, both with the operation `"watch"`. A non-local path gives
/// `Error::Unsupported`.
pub fn watch_dir(dir: &VPath, handler: DirEventHandler) -> Result<DirWatch> {
    let Some(local) = dir.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "watching needs a local path: {dir}"
        )));
    };
    let fail = |source: io::Error| Error::io("watch", dir.clone(), source);
    if !std::fs::metadata(&local).map_err(fail)?.is_dir() {
        return Err(fail(io::ErrorKind::NotADirectory.into()));
    }
    let backend = start(&local, handler).map_err(fail)?;
    Ok(DirWatch { _backend: backend })
}

/// Starts `notify` on `dir`. An event is a change to a direct child of `dir`, or to `dir`
/// itself.
#[cfg(not(windows))]
fn start(dir: &Path, mut handler: DirEventHandler) -> io::Result<Backend> {
    use notify::{RecursiveMode, Watcher};

    // FSEvents names the folder by its real path, so `/var/x` comes back as `/private/var/x`.
    let mut dirs = vec![dir.to_path_buf()];
    match std::fs::canonicalize(dir) {
        Ok(real) if real != dir => dirs.push(real),
        _ => {}
    }
    let mut ended = false;
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if ended {
            return;
        }
        let events = match result {
            Ok(event) => notify_backend::route(&event, &dirs),
            Err(_) => vec![DirEvent::Overflow],
        };
        for event in events {
            let last = event == DirEvent::Ended;
            handler(event);
            if last {
                ended = true;
                break;
            }
        }
    })
    .map_err(notify_to_io)?;
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .map_err(notify_to_io)?;
    Ok(watcher)
}

#[cfg(not(windows))]
fn notify_to_io(error: notify::Error) -> io::Error {
    match error.kind {
        notify::ErrorKind::PathNotFound => io::ErrorKind::NotFound.into(),
        notify::ErrorKind::Io(source) => source,
        _ => io::Error::other(error.to_string()),
    }
}
