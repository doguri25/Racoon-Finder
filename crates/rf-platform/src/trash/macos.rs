//! The macOS trash: Finder first, so that "Put Back" works, then `NSFileManager`; and the trash
//! as a place, read from `~/.Trash` and the `.Trashes/<uid>` folder of each other volume.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{
    NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask, NSString, NSURL,
};
use rf_core::{Error, Result, VPath};

use super::{TrashId, TrashItem, check_listed, directly_in, io_error, not_in_trash, stamp_of};

/// The file Finder keeps its view of a folder in, the trash's included; no trashed item.
const FINDER_FILE: &str = ".DS_Store";

/// Set once Finder has shown that it cannot be used in this process: automation was denied,
/// the request timed out, or there is no Finder. The next trash request then skips Finder
/// instead of waiting for the same failure again.
static FINDER_UNUSABLE: AtomicBool = AtomicBool::new(false);

/// How long to wait for `osascript` before it is killed and Finder counts as unusable.
const FINDER_DEADLINE: Duration = Duration::from_secs(20);

/// How often to look whether `osascript` has finished.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Parts of the `osascript` error text that mean Finder cannot be used at all: `-1743` the user
/// denied automation, `-1712` Finder did not answer in time, `-600` Finder is not running.
const FINDER_UNUSABLE_CODES: [&str; 3] = ["-1743", "-1712", "-600"];

/// The script, one line per `-e` argument. The path comes in as `argv`, so it needs no escaping,
/// and `delete` answers with the item's new place in the trash.
const SCRIPT: [&str; 6] = [
    "on run argv",
    "with timeout of 15 seconds",
    "tell application \"Finder\" to set trashed to delete (POSIX file (item 1 of argv) as alias)",
    "end timeout",
    "return POSIX path of (trashed as alias)",
    "end run",
];

/// Whether the user's trash folder can be found for the volume of `local`. A link counts for the
/// volume that holds the link, not the one it points to, because the link is what gets moved.
pub fn supported(local: &Path) -> bool {
    let probe = if is_symlink(local) {
        local.parent().unwrap_or(local)
    } else {
        local
    };
    autoreleasepool(|_| {
        let Some(url) = file_url(probe) else {
            return false;
        };
        NSFileManager::defaultManager()
            .URLForDirectory_inDomain_appropriateForURL_create_error(
                NSSearchPathDirectory::TrashDirectory,
                NSSearchPathDomainMask::UserDomainMask,
                Some(&url),
                false,
            )
            .is_ok()
    })
}

pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
    let Some(local) = path.as_local() else {
        return Err(Error::Unsupported(format!(
            "trash needs a local path: {path}"
        )));
    };
    // Finder resolves a link, so it would move what the link points to and leave the link
    // dangling. A link goes to `NSFileManager`, which moves the link itself.
    let link = is_symlink(local);
    autoreleasepool(|_| {
        if !link
            && !FINDER_UNUSABLE.load(Ordering::Relaxed)
            && let Finder::Trashed(location) = via_finder(local)
        {
            return Ok(location);
        }
        via_file_manager(local)
    })
    // Whoever reported success, the item that was asked for must be gone from its place;
    // otherwise something else moved, and the caller must not be told it worked.
    .and_then(|location| {
        if gone(local) {
            Ok(location)
        } else {
            Err(io::Error::other(
                "the trash reported success but the item is still there",
            ))
        }
    })
    .map_err(|e| Error::io("trash", path.clone(), e))
}

/// Whether `local` is a symbolic link itself, not whether it points to one.
fn is_symlink(local: &Path) -> bool {
    std::fs::symlink_metadata(local).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// The file URL of `local`, or `None` if the path is not valid UTF-8, which `NSString` needs.
fn file_url(local: &Path) -> Option<Retained<NSURL>> {
    Some(NSURL::fileURLWithPath(&NSString::from_str(local.to_str()?)))
}

/// What came of asking Finder to move an item to the trash.
enum Finder {
    /// The item is in the trash; the path is where, if Finder said.
    Trashed(Option<PathBuf>),
    /// Finder did not move the item, which is still where it was.
    Declined,
}

/// Asks Finder to move `local` to the trash.
fn via_finder(local: &Path) -> Finder {
    let mut command = Command::new("/usr/bin/osascript");
    for line in SCRIPT {
        command.arg("-e").arg(line);
    }
    let Ok(child) = command
        .arg(local)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Finder::Declined;
    };
    let stderr = match wait_for(child) {
        Some(output) if output.status.success() => {
            let reported = output.stdout.trim_ascii();
            return Finder::Trashed(
                (!reported.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(reported))),
            );
        }
        Some(output) => String::from_utf8_lossy(&output.stderr).into_owned(),
        None => {
            FINDER_UNUSABLE.store(true, Ordering::Relaxed);
            String::new()
        }
    };
    if FINDER_UNUSABLE_CODES
        .iter()
        .any(|code| stderr.contains(code))
    {
        FINDER_UNUSABLE.store(true, Ordering::Relaxed);
    }
    // Finder can move the item and then fail to name the new place.
    if gone(local) {
        Finder::Trashed(None)
    } else {
        Finder::Declined
    }
}

/// Waits for `child` for up to [`FINDER_DEADLINE`] and collects what it printed; `None` if it
/// had to be killed.
fn wait_for(mut child: Child) -> Option<Output> {
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < FINDER_DEADLINE => std::thread::sleep(POLL_INTERVAL),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    // The child has exited and both outputs are short, so reading them cannot block.
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_end(&mut stderr);
    }
    Some(Output {
        status,
        stdout,
        stderr,
    })
}

/// Whether `local` no longer exists.
fn gone(local: &Path) -> bool {
    std::fs::symlink_metadata(local).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
}

/// Moves `local` with `NSFileManager`. An item moved this way cannot be put back from Finder;
/// that is a limit of the API.
fn via_file_manager(local: &Path) -> io::Result<Option<PathBuf>> {
    let url = file_url(local).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path is not valid UTF-8, which the trash API needs",
        )
    })?;
    let mut resulting = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
        .map_err(|e| io::Error::other(e.localizedDescription().to_string()))?;
    Ok(resulting
        .and_then(|moved| moved.path())
        .map(|moved| PathBuf::from(moved.to_string())))
}

/// The user's trash folders: `~/.Trash` first, then `<volume>/.Trashes/<uid>` of each folder of
/// `/Volumes` that is another volume than `/` (which passes over the link to the system volume
/// there) and has one. The home trash is given even when it does not exist. A trash folder that is
/// itself a link is never one: it would make what it leads to the trash.
fn trash_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = dirs::home_dir()
        .map(|home| home.join(".Trash"))
        .filter(|trash| !is_symlink(trash))
        .into_iter()
        .collect();
    let uid = rustix::process::getuid().as_raw().to_string();
    let system = std::fs::metadata("/").map(|meta| meta.dev()).ok();
    let Ok(volumes) = std::fs::read_dir("/Volumes") else {
        return dirs;
    };
    let mut others: Vec<PathBuf> = volumes
        .flatten()
        .filter(|volume| !volume.file_name().as_bytes().starts_with(b"."))
        .filter(|volume| {
            std::fs::metadata(volume.path()).is_ok_and(|meta| Some(meta.dev()) != system)
        })
        .map(|volume| volume.path().join(".Trashes").join(&uid))
        .filter(|trash| std::fs::symlink_metadata(trash).is_ok_and(|meta| meta.is_dir()))
        .collect();
    others.sort();
    dirs.append(&mut others);
    dirs
}

/// The time `meta` last changed state (`st_ctime`), which a move into the trash sets.
fn changed_at(meta: &std::fs::Metadata) -> Option<SystemTime> {
    let seconds = u64::try_from(meta.ctime()).ok()?;
    let nanos = u32::try_from(meta.ctime_nsec()).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

/// Every item of the user's trash folders but Finder's own `.DS_Store`. The original place is
/// never known (see [`TrashItem::original`]). The home trash must be readable: macOS lets only an
/// app with Full Disk Access list it, and its error is the result; another volume's trash that
/// cannot be read is passed over.
pub fn list() -> Result<Vec<TrashItem>> {
    let mut items = Vec::new();
    if let Some(home) = dirs::home_dir().map(|home| home.join(".Trash"))
        && is_symlink(&home)
    {
        return Err(Error::Unsupported(format!(
            "{} is a link, not the trash, and is not used",
            home.display()
        )));
    }
    for (index, dir) in trash_dirs().into_iter().enumerate() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) if index == 0 => return Err(io_error("list_trash", &dir, e)),
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == FINDER_FILE {
                continue;
            }
            let location = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&location) else {
                continue;
            };
            let Ok(stamp) = stamp_of(&location, None) else {
                continue;
            };
            let is_dir = meta.is_dir();
            items.push(TrashItem {
                id: TrashId::of(&location),
                name,
                original: None,
                deleted: changed_at(&meta),
                size: (!is_dir).then_some(meta.len()),
                is_dir,
                location,
                stamp: Some(stamp),
            });
        }
    }
    Ok(items)
}

/// Refuses, with `Error::Unsupported`, an item that is not directly in one of the user's trash
/// folders, and Finder's `.DS_Store` there; then one that is not the item listed, by its
/// identity and change time (see `check_listed`; the macOS trash keeps no record).
fn check(item: &TrashItem) -> Result<()> {
    let finder_file = item.location.file_name() == Some(OsStr::new(FINDER_FILE));
    if finder_file || !directly_in(&item.location, &trash_dirs()) {
        return Err(not_in_trash(&item.location));
    }
    check_listed(item, None)
}

pub fn restore(item: &TrashItem, to: &Path) -> Result<()> {
    check(item)?;
    crate::fsops::rename_no_replace(&VPath::local(&item.location)?, &VPath::local(to)?)
}

/// Removes the item at `local` for good: a link itself, and anything else with `NSFileManager`,
/// which removes a folder with everything in it, following no link inside.
fn remove(local: &Path) -> Result<()> {
    // A link goes by `unlink` itself: `fileURLWithPath` would read a link to a folder as that
    // folder and add a `/`, which the removal must not follow.
    if is_symlink(local) {
        return std::fs::remove_file(local).map_err(|e| io_error("delete_forever", local, e));
    }
    autoreleasepool(|_| {
        let url = file_url(local).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the path is not valid UTF-8, which the file manager needs",
            )
        })?;
        NSFileManager::defaultManager()
            .removeItemAtURL_error(&url)
            .map_err(|e| io::Error::other(e.localizedDescription().to_string()))
    })
    .map_err(|e| io_error("delete_forever", local, e))
}

pub fn delete_forever(item: &TrashItem) -> Result<()> {
    check(item)?;
    remove(&item.location)
}

/// Removes every item of every trash folder with `NSFileManager`, Finder's `.DS_Store` aside. It
/// goes on after an error and gives the first one.
pub fn empty() -> Result<()> {
    let mut first = None;
    for item in list()? {
        if let Err(e) = remove(&item.location)
            && first.is_none()
        {
            first = Some(e);
        }
    }
    first.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link to a folder goes as the link; the folder and what is in it stay.
    #[test]
    fn a_link_to_a_folder_is_removed_itself() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("keep.txt"), "K").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&folder, &link).unwrap();
        remove(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert_eq!(
            std::fs::read_to_string(folder.join("keep.txt")).unwrap(),
            "K"
        );
        // A folder with a link in it goes, the link's target stays.
        let tree = dir.path().join("tree");
        std::fs::create_dir(&tree).unwrap();
        std::os::unix::fs::symlink(&folder, tree.join("inner-link")).unwrap();
        remove(&tree).unwrap();
        assert!(!tree.exists());
        assert_eq!(
            std::fs::read_to_string(folder.join("keep.txt")).unwrap(),
            "K"
        );
    }
}
