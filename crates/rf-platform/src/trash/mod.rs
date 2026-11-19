//! The system trash: sending items to it, which never turns into a permanent delete, and the
//! trash as a place (NAV-16): what it holds, putting an item back, deleting one for good and
//! emptying it.
//!
//! A request that the trash cannot serve (a network share, a USB stick, a path the Windows
//! recycle bin cannot take, a system without a trash) fails and leaves the item alone. Only the
//! caller, after asking the user, may delete permanently.
//!
//! The trashes are the user's own: on Windows the recycle bin of each fixed drive
//! (`<drive>\$Recycle.Bin\<user SID>`), on macOS `~/.Trash` and the `.Trashes/<uid>` folder of
//! each other mounted volume, on Linux the freedesktop home trash (`$XDG_DATA_HOME/Trash`).
//! Nothing outside their folders is ever deleted or moved: every function that acts on a
//! [`TrashItem`] first checks that its `location` lies directly in one of them, and refuses it
//! with `Error::Unsupported` otherwise. A folder is deleted with what is in it, but a link or a
//! junction in it is removed itself, never followed.
//!
//! An item is acted on only while it is the one that was listed. A trash reuses the names it
//! frees (on Linux and macOS a trashed item keeps its own name), so a row of an old listing can
//! name a place that another item has taken since. Each [`TrashItem`] of a listing carries its
//! [`TrashStamp`]: the identity and change time of the item itself and a digest of the trash's
//! record of it. Right before a restore or a delete for good the stamp is read again, and an item
//! whose stamp differs is refused with nothing done ([`is_changed_since_listed`]). What remains is
//! the time between that last look and the rename or removal itself, one system call: an item
//! swapped in at exactly that moment is not caught. Closing that needs calls made on an open
//! handle of the item (`unlinkat` relative to the trash folder, a delete by handle on Windows),
//! which std does not offer for a folder tree.

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use rf_core::{Error, Result, VPath};
use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
use self::linux as imp;
#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(windows)]
use self::windows as imp;

#[cfg(unix)]
mod freedesktop;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(unix)]
mod localtime;
#[cfg(target_os = "macos")]
mod macos;
mod parse;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use freedesktop::FreedesktopTrash;
pub use parse::{parse_recycle_index, parse_trashinfo};

/// Which item of a trash listing a row is. It is made from the item's place in the trash, so it
/// is unique in one listing and the same for the same item in the next one; its text holds no
/// `/`, so it can be one name of a path. Beyond that it means nothing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrashId(String);

impl TrashId {
    /// The id of the item at `location`, its place in the trash.
    pub fn of(location: &Path) -> TrashId {
        let text = VPath::local(location).map_or_else(
            |_| location.to_string_lossy().into_owned(),
            |path| path.to_uri(),
        );
        // `to_uri` writes `%` as `%25`, so a `/` written as `%2F` cannot be mistaken.
        TrashId(text.replace('/', "%2F"))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One item in the trash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashItem {
    pub id: TrashId,
    /// The name the item had before it was trashed (the last name of `original`), else its name
    /// in the trash.
    pub name: OsString,
    /// Where the item was trashed from, when the trash keeps it. Always `None` on macOS: the place
    /// Finder's "Put Back" uses is kept in Finder's own private records, which nothing else may
    /// read; there an item is put back by the app's undo record, or to a place the user picks.
    #[serde(with = "os_path_opt")]
    pub original: Option<PathBuf>,
    /// When it was trashed, when the trash keeps it (to the second on Linux). On macOS it is the
    /// time the item last changed state, which the move into the trash sets.
    pub deleted: Option<SystemTime>,
    /// The size in bytes, when known: a file's length, and on Windows a folder's whole size.
    pub size: Option<u64>,
    /// Whether the item is a folder (a link to one is not).
    pub is_dir: bool,
    /// Where the item is now, inside the trash. Every operation goes by this.
    #[serde(with = "os_path")]
    pub location: PathBuf,
    /// What the item was when it was listed; `None` for an item no listing made, which is never
    /// acted on.
    #[serde(default)]
    pub stamp: Option<TrashStamp>,
}

/// What identifies an item of the trash beyond its place, read when the trash is listed and
/// again right before the item is acted on: a reused name cannot match it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrashStamp {
    /// The volume and file IDs of the item itself, without following a link (`st_dev` and
    /// `st_ino` on Unix, the volume serial number and file ID on Windows), when the file system
    /// keeps usable ones.
    pub identity: Option<(u64, u128)>,
    /// When the item itself last changed state (`st_ctime`, seconds and nanoseconds), on Unix.
    pub changed: Option<(i64, i64)>,
    /// A digest (64-bit FNV-1a) of the bytes of the trash's record of the item: its `$I…` file on
    /// Windows, its `.trashinfo` file in a freedesktop trash (which holds its `Path=` and
    /// `DeletionDate=`). `None` without a readable one, and on macOS, which keeps none.
    pub record: Option<u64>,
}

/// The size above which a record is not read: none comes near it.
pub(crate) const MAX_RECORD_LEN: u64 = 64 * 1024 + 28;

/// The bytes of the regular file at `path`, when it is no larger than [`MAX_RECORD_LEN`].
pub(crate) fn read_record_bytes(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;

    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_RECORD_LEN {
        return None;
    }
    let file = crate::fsops::open_regular_file(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_LEN + 1).read_to_end(&mut bytes).ok()?;
    (u64::try_from(bytes.len()).ok()? <= MAX_RECORD_LEN).then_some(bytes)
}

/// The 64-bit FNV-1a digest of `bytes`: fixed, so that a stamp means the same in every build.
fn digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The stamp of the item at `location`, read now, with `record` the bytes of its record if the
/// trash keeps one. A missing item gives `ErrorKind::NotFound`.
pub(crate) fn stamp_of(location: &Path, record: Option<&[u8]>) -> Result<TrashStamp> {
    let path = VPath::local(location)?;
    let meta =
        std::fs::symlink_metadata(location).map_err(|e| Error::io("trash", path.clone(), e))?;
    let identity = match crate::fileid::file_identity(&path) {
        Ok((volume, file)) => Some((volume.0, file.0)),
        Err(e) if e.kind() == rf_core::ErrorKind::Unsupported => None,
        Err(e) => return Err(e),
    };
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt;
        Some((meta.ctime(), meta.ctime_nsec()))
    };
    #[cfg(not(unix))]
    let changed = {
        let _ = meta;
        None
    };
    Ok(TrashStamp {
        identity,
        changed,
        record: record.map(digest),
    })
}

/// Refuses `item` unless what is at its place now has the stamp it was listed with: the item and,
/// at `record`, the trash's record of it. A missing item gives `ErrorKind::NotFound`; any other
/// difference, or an item without a stamp, [`changed_since_listed`].
pub(crate) fn check_listed(item: &TrashItem, record: Option<&Path>) -> Result<()> {
    let bytes = record.and_then(read_record_bytes);
    let now = stamp_of(&item.location, bytes.as_deref())?;
    if item.stamp == Some(now) {
        Ok(())
    } else {
        Err(changed_since_listed(&item.location))
    }
}

impl TrashItem {
    /// The item at `location`, a folder when `is_dir`, of which nothing else is known: its id is
    /// that of the place, its name the place's last name.
    pub fn at(location: PathBuf, is_dir: bool) -> TrashItem {
        TrashItem {
            id: TrashId::of(&location),
            name: location
                .file_name()
                .map(ToOwned::to_owned)
                .unwrap_or_default(),
            original: None,
            deleted: None,
            size: None,
            is_dir,
            location,
            stamp: None,
        }
    }
}

/// Paths in a stored item, kept as the OS's own string so that no name is lost (a path that is
/// not UTF-8 cannot be stored as text).
mod os_path {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        path.as_os_str().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        OsString::deserialize(deserializer).map(PathBuf::from)
    }
}

/// [`os_path`] for a path that may be missing.
mod os_path_opt {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[allow(
        clippy::ref_option,
        reason = "the signature serde's `with` attribute calls"
    )]
    pub fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        path.as_deref().map(Path::as_os_str).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<OsString>::deserialize(deserializer).map(|path| path.map(PathBuf::from))
    }
}

/// Whether `path` lies where the system trash can take it.
///
/// On Windows that is a fixed local drive; network shares, removable and optical drives and
/// UNC paths give `false`. On macOS it is a volume for which the user's trash folder can be
/// found. On Linux it is the file system of the home trash, outside the temporary folders (see
/// the Linux module). Anywhere else, and for a non-local path, it is `false`. The item itself is
/// not looked at, beyond whether it is a link, so a path that does not exist gives the answer for
/// its volume.
pub fn trash_supported(path: &VPath) -> bool {
    path.to_extended_length()
        .is_some_and(|local| imp::supported(&local))
}

/// Moves `path` into the system trash.
///
/// `Ok(Some(location))` is the new place inside the trash, `Ok(None)` means the item went to
/// the trash but the system did not say where. On Windows the item goes to the recycle bin in
/// a way that "Restore" works, on macOS through Finder so that "Put Back" works, if Finder may
/// be automated, and on Linux to the home trash with its `.trashinfo` record, so that every
/// freedesktop file manager can put it back.
///
/// The item is never deleted permanently instead, and never swapped for another one: a
/// symbolic link is moved itself, not what it points to, and a name that the Windows shell
/// would read as a different item (`report.` as `report`) is refused. On Windows the shell's
/// item is matched to the requested one by file identity, so an item on a file system that
/// reports no usable file ID is refused too. A non-local path, a place without a trash, and an
/// item the trash will not take (a refused recycle, a redirected name, no file identity) give
/// `Error::Unsupported`: the trash cannot take this item, and deleting it permanently is left
/// to the caller, after asking the user. A missing item gives `ErrorKind::NotFound`, and
/// anything else is an I/O error of the trash itself (on Windows the shell may fail to read a
/// very long path, for example). In every error case the item is left where it was.
///
/// This call blocks. On Windows a refused recycle is tried once more after 300 ms if the same
/// item is still there, and on macOS the call can wait for Finder for up to 20 seconds.
pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "trash needs a local path: {path}"
        )));
    };
    std::fs::symlink_metadata(&local).map_err(|e| Error::io("trash", path.clone(), e))?;
    if !imp::supported(&local) {
        return Err(Error::Unsupported(format!(
            "no trash available for {path}; delete permanently instead"
        )));
    }
    imp::move_to_trash(path)
}

/// Puts an item back from the trash: moves it from `location`, its place inside the trash, to
/// `original`, the place it was trashed from.
///
/// This never replaces anything. If `original` is taken, by a file or a folder, the result is
/// `ErrorKind::AlreadyExists` and both items stay as they are. If the folder `original` was in
/// is gone, the result is `ErrorKind::NotFound`; on another volume it is
/// `ErrorKind::CrossesDevices`. A non-local path gives `Error::Unsupported`. An error carries
/// `location` as its path.
///
/// Once the item is back, the trash's record of it is deleted too. On Windows that is the `$I…`
/// file next to a `$R…` location that sits where the bin keeps its items,
/// `<root>\$Recycle.Bin\<user SID>\$R…`; elsewhere the `info/<name>.trashinfo` file of a
/// location in a `files` folder next to an `info` folder, as a freedesktop trash keeps them (the
/// Linux home trash, or one of a test's own). A failure there is ignored, because the item is
/// back either way. Any other pair of paths is renamed in the same way, and nothing else is
/// deleted.
pub fn restore_from_trash(location: &VPath, original: &VPath) -> Result<()> {
    crate::fsops::rename_no_replace(location, original)?;
    #[cfg(windows)]
    remove_recycle_record(location);
    #[cfg(unix)]
    if let Some(local) = location.as_local() {
        freedesktop::remove_info_record(local);
    }
    Ok(())
}

/// Every item in the user's trashes: on Windows the recycle bin of each fixed drive, on macOS
/// `~/.Trash` and the `.Trashes/<uid>` folder of each other mounted volume, on Linux the home
/// trash. The order is the trashes' own. A trash that does not exist holds nothing, and one that
/// cannot be read is passed over, except the home trash on macOS, whose error is the result
/// (macOS lets only an app with Full Disk Access read it). A system without a trash has none.
///
/// This call blocks: it reads every trash folder and, on Windows and Linux, every record.
pub fn list() -> Result<Vec<TrashItem>> {
    imp::list()
}

/// Puts `item` back at `to`, its `original` or a place the user chose, and removes the trash's
/// record of it (the `$I…` file on Windows, the `.trashinfo` file on Linux).
///
/// This never replaces anything: a taken `to` is `ErrorKind::AlreadyExists` and both items stay,
/// for the job to apply its conflict rule; a missing folder is `ErrorKind::NotFound` and another
/// volume `ErrorKind::CrossesDevices` (nothing is copied across). An item whose `location` is
/// not directly in one of the user's trash folders is refused with `Error::Unsupported` and
/// nothing is done.
pub fn restore(item: &TrashItem, to: &Path) -> Result<()> {
    imp::restore(item, to)
}

/// Deletes `item` for good: the item at its `location` (a folder with everything in it, a link
/// or junction itself and never what it leads to), then the trash's record of it. An item whose
/// `location` is not directly in one of the user's trash folders is refused with
/// `Error::Unsupported` and nothing is done.
pub fn delete_forever(item: &TrashItem) -> Result<()> {
    imp::delete_forever(item)
}

/// Whether the system empties its trash in one call that cannot be split into one call per item:
/// `SHEmptyRecycleBinW` on Windows. Elsewhere [`empty`] deletes item by item, and a job that
/// empties the trash does so itself with [`delete_forever`], so that each item that cannot be
/// deleted is a failure of its own, which a retry deletes alone.
pub const EMPTIES_AT_ONCE: bool = cfg!(windows);

/// Deletes everything in the user's trashes for good, by the system's own way where it has one:
/// `SHEmptyRecycleBinW` on Windows (with no question and no sound: the app asks first), each item
/// with `NSFileManager` on macOS, and on Linux the items, records and size cache of the home
/// trash. Nothing outside the trash folders is touched. The app must ask the user first: this
/// cannot be undone.
pub fn empty() -> Result<()> {
    imp::empty()
}

/// What marks the error of an item that is not the one that was listed (see
/// [`is_changed_since_listed`]).
#[derive(Debug)]
struct ChangedSinceListed;

impl std::fmt::Display for ChangedSinceListed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "the item in the trash is not the one that was listed; nothing was done (list the \
             trash again)",
        )
    }
}

impl std::error::Error for ChangedSinceListed {}

/// The error of an item that is in the trash folder but is not the one that was listed: an I/O
/// error (`ErrorKind::Other`) of `location` that [`is_changed_since_listed`] tells apart.
pub(crate) fn changed_since_listed(location: &Path) -> Error {
    io_error("trash", location, io::Error::other(ChangedSinceListed))
}

/// Whether `error` is the refusal of an item whose place in the trash now holds something else
/// than what the listing found there, or whose record changed: nothing was done, and the trash
/// should be listed again.
pub fn is_changed_since_listed(error: &Error) -> bool {
    matches!(error, Error::Io { source, .. }
        if source.get_ref().is_some_and(<dyn std::error::Error + Send + Sync>::is::<ChangedSinceListed>))
}

/// The error of an item that is not directly in a trash folder: `Error::Unsupported`, and
/// nothing was done to it.
pub(crate) fn not_in_trash(location: &Path) -> Error {
    Error::Unsupported(format!(
        "{} is not an item of the trash; nothing was done to it",
        location.display()
    ))
}

/// Whether `location` is an item directly in one of the folders `dirs`: a name that is neither
/// `.` nor `..`, in a folder that is one of them once both are resolved (links, `..`, letter
/// case on Windows), and no `..` anywhere in it. A folder that cannot be resolved holds nothing.
pub(crate) fn directly_in(location: &Path, dirs: &[PathBuf]) -> bool {
    let clean = location
        .components()
        .all(|component| !matches!(component, Component::ParentDir | Component::CurDir));
    let (Some(_), Some(parent)) = (location.file_name(), location.parent()) else {
        return false;
    };
    if !clean {
        return false;
    }
    let Ok(parent) = parent.canonicalize() else {
        return false;
    };
    dirs.iter()
        .filter_map(|dir| dir.canonicalize().ok())
        .any(|dir| dir == parent)
}

/// Removes the item at `path` for good, without following any link: a link (on Windows a
/// folder link or junction too) is removed itself, a folder with everything in it, where std's
/// `remove_dir_all` removes each link it meets inside as a link, and anything else as a file.
pub(crate) fn remove_tree(path: &Path) -> io::Result<()> {
    let file_type = std::fs::symlink_metadata(path)?.file_type();
    if file_type.is_symlink() {
        #[cfg(windows)]
        if std::os::windows::fs::FileTypeExt::is_symlink_dir(&file_type) {
            return std::fs::remove_dir(path);
        }
        std::fs::remove_file(path)
    } else if file_type.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// An I/O error of `op` on the local `path`.
pub(crate) fn io_error(op: &'static str, path: &Path, error: io::Error) -> Error {
    match VPath::local(path) {
        Ok(path) => Error::io(op, path, error),
        Err(e) => e.into(),
    }
}

/// Deletes the `$I…` record that belongs to the recycle bin item `location` (`$R…`), if
/// `location` lies in a user folder of a `$Recycle.Bin` folder (its grandparent). Errors are
/// ignored.
#[cfg(windows)]
fn remove_recycle_record(location: &VPath) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let Some(local) = location.to_extended_length() else {
        return;
    };
    let in_bin = local
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .is_some_and(|bin| bin.eq_ignore_ascii_case(OsStr::new("$Recycle.Bin")));
    let Some(name) = local.file_name().filter(|_| in_bin) else {
        return;
    };
    let units: Vec<u16> = name.encode_wide().collect();
    if let [dollar, r, rest @ ..] = units.as_slice()
        && (*dollar, *r) == (u16::from(b'$'), u16::from(b'R'))
    {
        let mut record = vec![u16::from(b'$'), u16::from(b'I')];
        record.extend_from_slice(rest);
        // The record is the bin's bookkeeping; without the item the shell ignores it.
        let _ = std::fs::remove_file(local.with_file_name(OsString::from_wide(&record)));
    }
}

/// Systems without a trash: nothing is supported, so `move_to_trash` stops before it gets here,
/// and the trash holds nothing.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use std::path::{Path, PathBuf};

    use rf_core::{Error, Result, VPath};

    use super::{TrashItem, not_in_trash};

    pub fn supported(_local: &Path) -> bool {
        false
    }

    pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
        Err(Error::Unsupported(format!(
            "no trash available for {path}; delete permanently instead"
        )))
    }

    pub fn list() -> Result<Vec<TrashItem>> {
        Ok(Vec::new())
    }

    pub fn restore(item: &TrashItem, _to: &Path) -> Result<()> {
        Err(not_in_trash(&item.location))
    }

    pub fn delete_forever(item: &TrashItem) -> Result<()> {
        Err(not_in_trash(&item.location))
    }

    pub fn empty() -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_one_name_and_tells_places_apart() {
        let dir = std::env::temp_dir();
        let a = TrashId::of(&dir.join("a"));
        let b = TrashId::of(&dir.join("b"));
        assert_ne!(a, b);
        assert_eq!(a, TrashId::of(&dir.join("a")));
        assert!(!a.as_str().contains('/'), "{}", a.as_str());
        VPath::new(rf_core::Scheme::Shell, "trash", &[a.as_str()]).unwrap();
        // A `/` in the text and a name holding `%2F` stay apart.
        assert_ne!(
            TrashId::of(&dir.join("x%2Fy")),
            TrashId::of(&dir.join("x").join("y"))
        );
    }

    #[test]
    fn an_item_of_which_only_the_place_is_known() {
        let location = std::env::temp_dir().join("문서 a.txt");
        let item = TrashItem::at(location.clone(), false);
        assert_eq!(item.name, OsString::from("문서 a.txt"));
        assert_eq!(item.id, TrashId::of(&location));
        assert_eq!((item.original, item.deleted, item.size), (None, None, None));
    }

    #[test]
    fn only_a_direct_item_of_a_trash_folder_is_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(bin.join("sub")).unwrap();
        std::fs::write(bin.join("a"), "A").unwrap();
        let dirs = [bin.clone()];
        assert!(directly_in(&bin.join("a"), &dirs));
        // Not there yet, but its place is in the trash.
        assert!(directly_in(&bin.join("b"), &dirs));
        for outside in [
            bin.clone(),
            bin.join("sub").join("x"),
            dir.path().join("a"),
            bin.join("..").join("a"),
            bin.join("sub").join("..").join("a"),
            bin.join("."),
            PathBuf::from("a"),
        ] {
            assert!(!directly_in(&outside, &dirs), "{outside:?}");
        }
        assert!(!directly_in(&bin.join("a"), &[dir.path().join("missing")]));
    }

    #[cfg(unix)]
    #[test]
    fn a_tree_is_removed_without_following_its_links() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), "K").unwrap();
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(tree.join("deep")).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("deep").join("link")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("top-link")).unwrap();
        remove_tree(&tree).unwrap();
        remove_tree(&dir.path().join("top-link")).unwrap();
        assert!(!tree.exists());
        assert!(std::fs::symlink_metadata(dir.path().join("top-link")).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("keep")).unwrap(), "K");
    }
}
