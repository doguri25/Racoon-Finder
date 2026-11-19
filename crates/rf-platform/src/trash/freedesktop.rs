//! A trash kept as the freedesktop.org Trash specification (1.0) lays it out, in one folder of
//! its own: `files/` holds the trashed items and `info/` one `<name>.trashinfo` record for each,
//! with its original path and the time it was trashed.
//!
//! On Linux the home trash (`$XDG_DATA_HOME/Trash`) is one; tests use one in a temporary
//! folder. Nothing here removes or moves anything that is not in `files/`, `info/` or the
//! `directorysizes` cache of the trash it was given, and no link is ever followed: a `files/` or
//! `info/` that is itself a link makes the trash unusable (`Error::Unsupported`).

use std::ffi::{OsStr, OsString};
use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rf_core::{Error, ErrorKind, Result, VPath};

use super::parse::{escape_path, format_deletion_date, parse_trashinfo};
use super::{
    TrashId, TrashItem, check_listed, io_error, localtime, not_in_trash, read_record_bytes,
    remove_tree, stamp_of,
};

/// The ending of every record in `info/`.
const INFO_SUFFIX: &str = ".trashinfo";

/// How many names a trash tries for one item before it gives up (`a.txt`, `a.2.txt`, …).
const MAX_NAMES: u32 = 10_000;

/// One freedesktop trash: the folder `root` with its `files/` and `info/`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FreedesktopTrash {
    root: PathBuf,
}

impl FreedesktopTrash {
    /// The trash in the folder `root`, which need not exist yet: the first item sent to it makes
    /// it, with `files/` and `info/`, readable by the user alone. `root` should be an absolute
    /// path.
    pub fn new(root: impl Into<PathBuf>) -> FreedesktopTrash {
        FreedesktopTrash { root: root.into() }
    }

    /// The folder of the trash.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder that holds the trashed items.
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    /// The folder that holds the records of the trashed items.
    pub fn info_dir(&self) -> PathBuf {
        self.root.join("info")
    }

    /// Whether this trash takes items from the place `local`, an item or the folder a window
    /// shows: it is on the same file system as the trash (the trash's nearest folder that exists,
    /// when it is not made yet), and it is neither the trash nor inside it. A folder above the
    /// trash (the home folder) counts: its items can go, though the folder itself never does (see
    /// [`move_to_trash`](Self::move_to_trash)). The place itself is looked at without following a
    /// link; when it does not exist, its folder is.
    pub fn takes(&self, local: &Path) -> bool {
        if local.starts_with(&self.root) {
            return false;
        }
        let item = std::fs::symlink_metadata(local)
            .or_else(|_| std::fs::metadata(local.parent().unwrap_or(local)));
        let trash = self
            .root
            .ancestors()
            .find_map(|dir| std::fs::metadata(dir).ok());
        matches!((item, trash), (Ok(item), Some(trash)) if item.dev() == trash.dev())
    }

    /// Moves the item at `path`, a local path, into the trash, as the specification orders: the
    /// record `info/<name>.trashinfo` is made first, with `O_EXCL` so that two items never share
    /// a name, and then the item is renamed to `files/<name>`, never replacing anything. The name
    /// is the item's own, and `a.2.txt`, `a.3.txt`, … while that is taken in either folder. The
    /// record holds the absolute path and the local time.
    ///
    /// Gives `Ok(Some(location))`, the item's place in `files/`. A link is moved itself. A path
    /// that [`takes`](Self::takes) refuses, a folder that holds the trash (it would take the trash
    /// with it), or a rename across file systems (two mounts of one), gives `Error::Unsupported`
    /// and leaves the item where it was; so does any other error, which takes the record away
    /// again.
    pub fn move_to_trash(&self, path: &VPath) -> Result<Option<PathBuf>> {
        let Some(local) = path.as_local() else {
            return Err(Error::Unsupported(format!(
                "trash needs a local path: {path}"
            )));
        };
        std::fs::symlink_metadata(local).map_err(|e| Error::io("trash", path.clone(), e))?;
        if !self.takes(local) || self.root.starts_with(local) {
            return Err(Error::Unsupported(format!(
                "the trash {} cannot take {path}; delete permanently instead",
                self.root.display()
            )));
        }
        let Some(name) = local.file_name() else {
            return Err(Error::Unsupported(format!(
                "cannot trash a volume root: {path}"
            )));
        };
        self.check_folders()?;
        let (files, info) = (self.files_dir(), self.info_dir());
        for dir in [&files, &info] {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| io_error("trash", dir, e))?;
        }
        let record = record_text(local, SystemTime::now());
        for n in 1..=MAX_NAMES {
            let candidate = numbered(name, n);
            let info_path = info.join(info_name(&candidate));
            match write_record(&info_path, &record) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) if e.raw_os_error() == Some(libc::ENAMETOOLONG) => {
                    return Err(Error::Unsupported(format!(
                        "the name of {path} is too long for the trash to keep its record; delete \
                         permanently instead"
                    )));
                }
                Err(e) => return Err(Error::io("trash", path.clone(), e)),
            }
            let location = files.join(&candidate);
            let target = VPath::local(&location)?;
            match crate::fsops::rename_no_replace(path, &target) {
                Ok(()) => {
                    // The item is in the trash either way; these only make it durable sooner.
                    let _ = crate::fsops::sync_dir(&files);
                    let _ = crate::fsops::sync_dir(&info);
                    return Ok(Some(location));
                }
                Err(e) => {
                    // The record names no item now: it goes.
                    let _ = std::fs::remove_file(&info_path);
                    match e.kind() {
                        // A stale item in `files/` has the name: the next one is tried.
                        ErrorKind::AlreadyExists => {}
                        ErrorKind::CrossesDevices => {
                            return Err(Error::Unsupported(format!(
                                "the trash {} is on another file system than {path}; delete \
                                 permanently instead",
                                self.root.display()
                            )));
                        }
                        _ => return Err(e),
                    }
                }
            }
        }
        let error = io::Error::new(io::ErrorKind::AlreadyExists, "no free name in the trash");
        Err(Error::io("trash", path.clone(), error))
    }

    /// Every item in `files/`, with what its record says: the original path (a relative one read
    /// from the folder the trash is in) and the time it was trashed. An item whose record is
    /// missing, larger than 64 KiB or malformed is listed too, with neither, as the specification
    /// wants such an item shown; a record without an item is not listed. The name is the last
    /// name of the original path, else the item's name in the trash. A file's size is its length;
    /// a folder's is not known (`None`). Each item carries its stamp: its identity and change
    /// time and a digest of its record, which a restore or a delete checks again.
    ///
    /// A trash that does not exist yet is empty. The order is that of the folder.
    pub fn list(&self) -> Result<Vec<TrashItem>> {
        self.check_folders()?;
        let files = self.files_dir();
        let entries = match std::fs::read_dir(&files) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error("list_trash", &files, e)),
        };
        let base = self.root.parent().unwrap_or(&self.root);
        let mut items = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_error("list_trash", &files, e))?;
            let in_trash = entry.file_name();
            let location = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&location) else {
                continue;
            };
            let bytes = read_record_bytes(&self.info_dir().join(info_name(&in_trash)));
            let Ok(stamp) = stamp_of(&location, bytes.as_deref()) else {
                continue;
            };
            let record = bytes
                .as_deref()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .and_then(parse_trashinfo);
            let original = record.as_ref().map(|(path, _)| base.join(path));
            let name = original
                .as_deref()
                .and_then(Path::file_name)
                .map_or_else(|| in_trash.clone(), OsStr::to_os_string);
            let is_dir = meta.is_dir();
            items.push(TrashItem {
                id: TrashId::of(&location),
                name,
                original,
                deleted: record.and_then(|(_, deleted)| deleted),
                size: (!is_dir).then_some(meta.len()),
                is_dir,
                location,
                stamp: Some(stamp),
            });
        }
        Ok(items)
    }

    /// Puts `item` back at `to`, never replacing anything (see `fsops::rename_no_replace`: a taken
    /// `to` is `ErrorKind::AlreadyExists`, a missing folder `NotFound`, another file system
    /// `CrossesDevices`), and then removes its record. An item that is not directly in `files/`
    /// of this trash is refused with `Error::Unsupported` and nothing is done.
    pub fn restore(&self, item: &TrashItem, to: &Path) -> Result<()> {
        self.check(item)?;
        crate::fsops::rename_no_replace(&VPath::local(&item.location)?, &VPath::local(to)?)?;
        remove_info_record(&item.location);
        Ok(())
    }

    /// Removes `item` from the trash for good: the item (a folder with everything in it, a link
    /// itself and never what it leads to), then its record. An item that is not directly in
    /// `files/` of this trash is refused with `Error::Unsupported` and nothing is done. An error
    /// of the removal keeps the record.
    pub fn delete_forever(&self, item: &TrashItem) -> Result<()> {
        self.check(item)?;
        remove_tree(&item.location).map_err(|e| io_error("delete_forever", &item.location, e))?;
        remove_info_record(&item.location);
        Ok(())
    }

    /// Removes everything in `files/` for good as [`delete_forever`](Self::delete_forever) does,
    /// every `*.trashinfo` record in `info/`, and the `directorysizes` cache: the trash's folders
    /// stay, empty. Nothing else is touched, whatever the links in it lead to. It goes on after
    /// an error and gives the first one; a trash that does not exist is empty already.
    pub fn empty(&self) -> Result<()> {
        self.check_folders()?;
        let mut first = None;
        let mut keep = |result: Result<()>| {
            if let Err(e) = result
                && first.is_none()
            {
                first = Some(e);
            }
        };
        for (dir, records_only) in [(self.files_dir(), false), (self.info_dir(), true)] {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    keep(Err(io_error("empty_trash", &dir, e)));
                    continue;
                }
            };
            for entry in entries {
                let path = match entry {
                    Ok(entry) => entry.path(),
                    Err(e) => {
                        keep(Err(io_error("empty_trash", &dir, e)));
                        continue;
                    }
                };
                if records_only && !is_record(&path) {
                    continue;
                }
                keep(remove_tree(&path).map_err(|e| io_error("empty_trash", &path, e)));
            }
        }
        let sizes = self.root.join("directorysizes");
        match std::fs::remove_file(&sizes) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                keep(Err(io_error("empty_trash", &sizes, e)));
            }
            _ => {}
        }
        first.map_or(Ok(()), Err)
    }

    /// Refuses, with `Error::Unsupported`, a trash whose `files/` or `info/` is a link (to the
    /// user's documents, say), which would make what it leads to the trash: nothing is listed,
    /// deleted, emptied or moved through it.
    fn check_folders(&self) -> Result<()> {
        for dir in [self.files_dir(), self.info_dir()] {
            if std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(Error::Unsupported(format!(
                    "{} is a link, not a folder of the trash, and is not used",
                    dir.display()
                )));
            }
        }
        Ok(())
    }

    /// Refuses, with `Error::Unsupported`, an item that is not directly in `files/`, and then one
    /// that is not the item listed, by the item and its record (see `check_listed`).
    fn check(&self, item: &TrashItem) -> Result<()> {
        self.check_folders()?;
        if !super::directly_in(&item.location, &[self.files_dir()]) {
            return Err(not_in_trash(&item.location));
        }
        let name = item.location.file_name().unwrap_or_default();
        check_listed(item, Some(&self.info_dir().join(info_name(name))))
    }
}

/// The name of the record of the item called `name` in `files/`: `name.trashinfo`.
fn info_name(name: &OsStr) -> OsString {
    let mut record = name.to_os_string();
    record.push(INFO_SUFFIX);
    record
}

/// Whether `path` is named as a record is (`*.trashinfo`).
fn is_record(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().ends_with(INFO_SUFFIX.as_bytes()))
}

/// `name` for the `n`th try: as it is first, then with `.n` before its extension (`a.2.txt`,
/// `archive.2.tar.gz`, `.bashrc.2`), as `GLib` numbers its trash names.
fn numbered(name: &OsStr, n: u32) -> OsString {
    if n == 1 {
        return name.to_os_string();
    }
    let bytes = name.as_encoded_bytes();
    // The first dot that is not the name's first character starts the extension.
    let split = bytes
        .iter()
        .skip(1)
        .position(|&b| b == b'.')
        .map_or(bytes.len(), |at| at + 1);
    let (stem, extension) = bytes.split_at(split);
    let mut numbered = Vec::with_capacity(bytes.len() + 6);
    numbered.extend_from_slice(stem);
    numbered.extend_from_slice(format!(".{n}").as_bytes());
    numbered.extend_from_slice(extension);
    OsString::from_vec(numbered)
}

/// The text of the record of an item trashed from `original` at `now`. A time the system cannot
/// put in local time is left out, which only makes it unknown.
fn record_text(original: &Path, now: SystemTime) -> String {
    let date = now
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .and_then(localtime::utc_to_local)
        .map(|local| format!("DeletionDate={}\n", format_deletion_date(local)))
        .unwrap_or_default();
    format!("[Trash Info]\nPath={}\n{date}", escape_path(original))
}

/// Makes the record `path` with `text`, only if nothing has that name (`O_EXCL`), readable by
/// the user alone, and makes it durable. A record that cannot be finished is taken away again.
fn write_record(path: &Path, text: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| crate::fsops::sync_file(&file));
    if written.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    written
}

/// The record of the item at `location` when `location` is in a folder `files`, as a
/// freedesktop trash keeps it: `info/<name>.trashinfo` beside `files`. `None` for any other
/// place.
pub(super) fn record_path(location: &Path) -> Option<PathBuf> {
    let (name, files) = (location.file_name()?, location.parent()?);
    if files.file_name() != Some(OsStr::new("files")) {
        return None;
    }
    Some(files.parent()?.join("info").join(info_name(name)))
}

/// Removes the record of the item at `location` when `location` is in a folder `files` next to
/// a folder `info`, the freedesktop layout (see [`record_path`]): `info/<name>.trashinfo` goes,
/// and nothing else. Errors are ignored: the item is out of the trash, and a record without an
/// item is not listed.
pub(super) fn remove_info_record(location: &Path) {
    let Some(record) = record_path(location) else {
        return;
    };
    let is_file = std::fs::symlink_metadata(&record).is_ok_and(|meta| meta.is_file());
    if is_file {
        let _ = std::fs::remove_file(record);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use super::*;

    /// A trash in a new temporary folder, and the folder (canonical, as the trash reports
    /// places) where the test keeps its own items.
    fn fixture() -> (tempfile::TempDir, FreedesktopTrash, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let trash = FreedesktopTrash::new(base.join("data").join("Trash"));
        let home = base.join("home");
        std::fs::create_dir(&home).unwrap();
        (dir, trash, home)
    }

    fn local(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    fn trash_file(trash: &FreedesktopTrash, path: &Path, text: &str) -> PathBuf {
        std::fs::write(path, text).unwrap();
        trash.move_to_trash(&local(path)).unwrap().unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn an_item_goes_in_with_its_record_and_comes_back_out() {
        let (_dir, trash, home) = fixture();
        let file = home.join("a b.txt");
        let before = SystemTime::now();
        let location = trash_file(&trash, &file, "A");
        assert!(!file.exists());
        assert_eq!(location, trash.files_dir().join("a b.txt"));
        assert_eq!(std::fs::read_to_string(&location).unwrap(), "A");
        let record = std::fs::read_to_string(trash.info_dir().join("a b.txt.trashinfo")).unwrap();
        assert!(record.starts_with("[Trash Info]\nPath="), "{record}");
        assert!(record.contains("a%20b.txt"), "{record}");
        // The trash is the user's own.
        let mode = std::fs::metadata(trash.root()).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let [item] = trash.list().unwrap().try_into().unwrap();
        assert_eq!(item.name, OsString::from("a b.txt"));
        assert_eq!(item.original.as_deref(), Some(file.as_path()));
        assert_eq!(item.location, location);
        assert_eq!((item.size, item.is_dir), (Some(1), false));
        // To the second: the record keeps no more.
        let deleted = item.deleted.expect("a deletion time");
        let floor = before - std::time::Duration::from_secs(1);
        assert!(
            deleted >= floor && deleted <= SystemTime::now(),
            "{deleted:?}"
        );
        assert_eq!(item.id, TrashId::of(&location));

        trash.restore(&item, &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
        assert_eq!(names(&trash.files_dir()), Vec::<String>::new());
        assert_eq!(names(&trash.info_dir()), Vec::<String>::new());
    }

    #[test]
    fn items_of_one_name_get_names_of_their_own() {
        let (_dir, trash, home) = fixture();
        for text in ["1", "2", "3"] {
            trash_file(&trash, &home.join("a.tar.gz"), text);
        }
        std::fs::create_dir(home.join(".hidden")).unwrap();
        trash.move_to_trash(&local(&home.join(".hidden"))).unwrap();
        std::fs::create_dir(home.join(".hidden")).unwrap();
        trash.move_to_trash(&local(&home.join(".hidden"))).unwrap();
        assert_eq!(
            names(&trash.files_dir()),
            [
                ".hidden",
                ".hidden.2",
                "a.2.tar.gz",
                "a.3.tar.gz",
                "a.tar.gz"
            ]
        );
        assert_eq!(
            names(&trash.info_dir()),
            [
                ".hidden.2.trashinfo",
                ".hidden.trashinfo",
                "a.2.tar.gz.trashinfo",
                "a.3.tar.gz.trashinfo",
                "a.tar.gz.trashinfo"
            ]
        );
        // Every one remembers where it came from.
        let mut originals: Vec<PathBuf> = trash
            .list()
            .unwrap()
            .into_iter()
            .map(|item| item.original.unwrap())
            .collect();
        originals.sort();
        originals.dedup();
        assert_eq!(originals, [home.join(".hidden"), home.join("a.tar.gz")]);
    }

    /// A stale item in `files/` without a record keeps its name; the new item takes another.
    #[test]
    fn a_name_taken_in_either_folder_is_passed_over() {
        let (_dir, trash, home) = fixture();
        std::fs::create_dir_all(trash.files_dir()).unwrap();
        std::fs::create_dir_all(trash.info_dir()).unwrap();
        std::fs::write(trash.files_dir().join("a.txt"), "stale").unwrap();
        std::fs::write(trash.info_dir().join("a.2.txt.trashinfo"), "stale").unwrap();
        let location = trash_file(&trash, &home.join("a.txt"), "new");
        assert_eq!(location, trash.files_dir().join("a.3.txt"));
        assert_eq!(
            std::fs::read_to_string(trash.files_dir().join("a.txt")).unwrap(),
            "stale"
        );
        assert_eq!(
            names(&trash.info_dir()),
            ["a.2.txt.trashinfo", "a.3.txt.trashinfo"]
        );
    }

    #[test]
    fn a_restore_never_replaces_anything() {
        let (_dir, trash, home) = fixture();
        let file = home.join("a.txt");
        trash_file(&trash, &file, "old");
        std::fs::write(&file, "new").unwrap();
        let [item] = trash.list().unwrap().try_into().unwrap();
        let error = trash.restore(&item, &file).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&item.location).unwrap(), "old");
        assert_eq!(names(&trash.info_dir()), ["a.txt.trashinfo"]);
        // Somewhere else it goes.
        trash.restore(&item, &home.join("b.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(home.join("b.txt")).unwrap(), "old");
        assert_eq!(names(&trash.info_dir()), Vec::<String>::new());
    }

    /// Deleting a trashed folder for good removes the folder and its record and nothing else: not
    /// what a link in it leads to, not another trashed item.
    #[test]
    fn delete_forever_removes_only_the_item_and_its_record() {
        let (_dir, trash, home) = fixture();
        let outside = home.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "K").unwrap();
        let folder = home.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inner.txt"), "I").unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("link")).unwrap();
        trash.move_to_trash(&local(&folder)).unwrap();
        trash_file(&trash, &home.join("other.txt"), "O");

        let items = trash.list().unwrap();
        let folder_item = items.iter().find(|i| i.name == "folder").unwrap();
        assert!(folder_item.is_dir);
        assert_eq!(folder_item.size, None);
        trash.delete_forever(folder_item).unwrap();

        assert_eq!(
            std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "K"
        );
        assert_eq!(names(&trash.files_dir()), ["other.txt"]);
        assert_eq!(names(&trash.info_dir()), ["other.txt.trashinfo"]);
    }

    /// A link put in the trash goes as a link; emptying the trash leaves what it leads to.
    #[test]
    fn empty_leaves_nothing_outside_the_trash_touched() {
        let (_dir, trash, home) = fixture();
        let outside = home.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "K").unwrap();
        std::os::unix::fs::symlink(&outside, home.join("link")).unwrap();
        trash.move_to_trash(&local(&home.join("link"))).unwrap();
        trash_file(&trash, &home.join("a.txt"), "A");
        std::fs::write(trash.root().join("directorysizes"), "0 0 x\n").unwrap();
        // Something of another app's in `info/` that is no record stays.
        std::fs::write(trash.info_dir().join("README"), "not a record").unwrap();
        let link_item = trash
            .list()
            .unwrap()
            .into_iter()
            .find(|i| i.name == "link")
            .unwrap();
        assert!(!link_item.is_dir);

        trash.empty().unwrap();

        assert_eq!(names(&trash.files_dir()), Vec::<String>::new());
        assert_eq!(names(&trash.info_dir()), ["README"]);
        assert!(!trash.root().join("directorysizes").exists());
        assert_eq!(
            std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "K"
        );
        assert_eq!(names(&home), ["outside"]);
        // An empty trash, and one not made yet, empty without a word.
        trash.empty().unwrap();
        FreedesktopTrash::new(home.join("none")).empty().unwrap();
        assert_eq!(FreedesktopTrash::new(home.join("none")).list().unwrap(), []);
    }

    #[test]
    fn a_place_outside_the_trash_is_refused() {
        let (_dir, trash, home) = fixture();
        trash_file(&trash, &home.join("a.txt"), "A");
        let victim = home.join("victim.txt");
        std::fs::write(&victim, "V").unwrap();
        let deep = trash.files_dir().join("sub");
        std::fs::create_dir(&deep).unwrap();
        std::fs::write(deep.join("x"), "X").unwrap();
        for location in [
            victim.clone(),
            trash.info_dir().join("a.txt.trashinfo"),
            trash.root().to_path_buf(),
            trash.files_dir(),
            deep.join("x"),
            trash
                .files_dir()
                .join("..")
                .join("info")
                .join("a.txt.trashinfo"),
        ] {
            let mut item = TrashItem::at(location.clone(), false);
            item.original = Some(home.join("back.txt"));
            for result in [
                trash.delete_forever(&item),
                trash.restore(&item, &home.join("back.txt")),
            ] {
                assert_eq!(
                    result.unwrap_err().kind(),
                    ErrorKind::Unsupported,
                    "{location:?}"
                );
            }
        }
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "V");
        assert!(trash.info_dir().join("a.txt.trashinfo").exists());
        assert!(deep.join("x").exists());
        assert!(!home.join("back.txt").exists());
        // Another trash's item is outside this one.
        let other = FreedesktopTrash::new(home.join("Trash2"));
        let location = trash_file(&other, &home.join("b.txt"), "B");
        let [item] = other.list().unwrap().try_into().unwrap();
        assert_eq!(
            trash.delete_forever(&item).unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        assert!(location.exists());
    }

    #[test]
    fn the_trash_never_takes_itself_or_what_holds_it() {
        let (_dir, trash, home) = fixture();
        trash_file(&trash, &home.join("a.txt"), "A");
        // Nothing in the trash can be trashed again.
        for path in [trash.root().to_path_buf(), trash.files_dir().join("a.txt")] {
            assert!(!trash.takes(&path), "{path:?}");
        }
        // The folders above it take items, but are never moved into it themselves.
        for path in [
            trash.root().to_path_buf(),
            trash.files_dir().join("a.txt"),
            trash.root().parent().unwrap().to_path_buf(),
        ] {
            let error = trash.move_to_trash(&local(&path)).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Unsupported, "{path:?}");
            assert!(path.exists());
        }
        assert!(trash.takes(&home.join("not-there-yet")));
        assert_eq!(
            trash
                .move_to_trash(&local(&home.join("missing")))
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
    }

    /// A home folder with the home trash in it: the home and every folder above the trash take
    /// their items (`trash_supported` is asked for the folder a window shows), and none of those
    /// folders goes into the trash itself.
    #[test]
    fn the_folders_above_the_trash_take_their_items() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("kim");
        let trash = FreedesktopTrash::new(home.join(".local").join("share").join("Trash"));
        std::fs::create_dir_all(trash.root()).unwrap();
        let above = [
            home.clone(),
            home.join(".local"),
            home.join(".local").join("share"),
        ];
        for folder in &above {
            assert!(trash.takes(folder), "{folder:?}");
            let error = trash.move_to_trash(&local(folder)).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Unsupported, "{folder:?}");
            assert!(folder.exists());
        }
        let notes = home.join("notes.txt");
        let location = trash_file(&trash, &notes, "N");
        assert!(!notes.exists());
        assert_eq!(location, trash.files_dir().join("notes.txt"));
    }

    /// A file system other than the trash's is refused: `/dev/shm` is a `tmpfs` of its own on
    /// Linux, where it exists.
    #[test]
    fn another_file_system_is_refused() {
        let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
            eprintln!("skipped: no /dev/shm here");
            return;
        };
        let (_dir, trash, _home) = fixture();
        let file = other.path().join("a.txt");
        std::fs::write(&file, "A").unwrap();
        let same = std::fs::metadata(other.path()).unwrap().dev()
            == std::fs::metadata(trash.root().ancestors().nth(2).unwrap())
                .unwrap()
                .dev();
        if same {
            eprintln!("skipped: /dev/shm is on the temporary folder's file system");
            return;
        }
        assert!(!trash.takes(&file));
        assert_eq!(
            trash.move_to_trash(&local(&file)).unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        assert!(file.exists());
    }

    #[test]
    fn a_record_that_cannot_be_read_still_lists_its_item() {
        let (_dir, trash, home) = fixture();
        trash_file(&trash, &home.join("a.txt"), "A");
        trash_file(&trash, &home.join("b.txt"), "B");
        trash_file(&trash, &home.join("c.txt"), "C");
        std::fs::write(trash.info_dir().join("a.txt.trashinfo"), "garbage").unwrap();
        std::fs::remove_file(trash.info_dir().join("b.txt.trashinfo")).unwrap();
        let big = format!("[Trash Info]\nPath=/c.txt\n#{}\n", "x".repeat(70 * 1024));
        std::fs::write(trash.info_dir().join("c.txt.trashinfo"), big).unwrap();
        // A record of an item that is gone lists nothing.
        std::fs::write(
            trash.info_dir().join("gone.txt.trashinfo"),
            "[Trash Info]\nPath=/gone.txt\n",
        )
        .unwrap();
        let mut items = trash.list().unwrap();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        let seen: Vec<_> = items
            .iter()
            .map(|i| {
                (
                    i.name.to_string_lossy().into_owned(),
                    i.original.clone(),
                    i.deleted,
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("a.txt".to_owned(), None, None),
                ("b.txt".to_owned(), None, None),
                ("c.txt".to_owned(), None, None)
            ]
        );
    }

    /// A relative path is read from the folder the trash is in.
    #[test]
    fn a_relative_original_is_read_from_the_folder_of_the_trash() {
        let (_dir, trash, home) = fixture();
        trash_file(&trash, &home.join("a.txt"), "A");
        std::fs::write(
            trash.info_dir().join("a.txt.trashinfo"),
            "[Trash Info]\nPath=docs/a.txt\nDeletionDate=2020-01-02T03:04:05\n",
        )
        .unwrap();
        let [item] = trash.list().unwrap().try_into().unwrap();
        let base = trash.root().parent().unwrap();
        assert_eq!(item.original, Some(base.join("docs/a.txt")));
        assert!(item.deleted.is_some());
    }

    /// Where the file system takes a name that is not UTF-8 (APFS refuses one with `EILSEQ`).
    #[test]
    fn a_name_that_is_not_utf8_survives_the_round_trip() {
        let (_dir, trash, home) = fixture();
        let file = home.join(OsStr::from_bytes(b"caf\xE9.txt"));
        if let Err(e) = std::fs::write(&file, "A") {
            assert_eq!(e.raw_os_error(), Some(libc::EILSEQ), "{e}");
            eprintln!("skipped: this file system takes no name that is not UTF-8 ({e})");
            return;
        }
        let location = trash.move_to_trash(&local(&file)).unwrap().unwrap();
        assert_eq!(location.file_name(), file.file_name());
        let [item] = trash.list().unwrap().try_into().unwrap();
        assert_eq!(item.original.as_deref(), Some(file.as_path()));
        trash.restore(&item, &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    }

    /// A row listed before its name was taken by another trashed item never touches that item:
    /// it is put back elsewhere, and another file of the same name is trashed in its place.
    #[test]
    fn a_stale_item_never_touches_the_item_that_took_its_place() {
        let (_dir, trash, home) = fixture();
        for folder in ["x", "y"] {
            std::fs::create_dir(home.join(folder)).unwrap();
        }
        trash_file(&trash, &home.join("x").join("a.txt"), "first");
        let [stale] = trash.list().unwrap().try_into().unwrap();
        trash
            .restore(&stale, &home.join("x").join("a.txt"))
            .unwrap();
        let location = trash_file(&trash, &home.join("y").join("a.txt"), "second");
        assert_eq!(location, stale.location, "the name is taken again");

        let elsewhere = home.join("z.txt");
        for result in [
            trash.delete_forever(&stale),
            trash.restore(&stale, &elsewhere),
        ] {
            let error = result.unwrap_err();
            assert!(super::super::is_changed_since_listed(&error), "{error}");
        }
        assert_eq!(std::fs::read_to_string(&location).unwrap(), "second");
        assert_eq!(names(&trash.info_dir()), ["a.txt.trashinfo"]);
        assert!(!elsewhere.exists());

        // An item made by hand, which no listing vouches for, is refused the same way.
        let unlisted = TrashItem::at(location.clone(), false);
        let error = trash.delete_forever(&unlisted).unwrap_err();
        assert!(super::super::is_changed_since_listed(&error), "{error}");

        // The item as it is listed now goes.
        let [fresh] = trash.list().unwrap().try_into().unwrap();
        assert_eq!(fresh.original, Some(home.join("y").join("a.txt")));
        trash.delete_forever(&fresh).unwrap();
        assert_eq!(names(&trash.files_dir()), Vec::<String>::new());
    }

    /// An item whose record was rewritten, or that was swapped for another under its name, is no
    /// longer the one listed.
    #[test]
    fn a_changed_item_or_record_is_not_the_one_listed() {
        let (_dir, trash, home) = fixture();
        trash_file(&trash, &home.join("a.txt"), "A");
        trash_file(&trash, &home.join("b.txt"), "B");
        let items = trash.list().unwrap();
        let item = |name: &str| items.iter().find(|i| i.name == name).unwrap().clone();
        let (a, b) = (item("a.txt"), item("b.txt"));
        std::fs::write(
            trash.info_dir().join("a.txt.trashinfo"),
            "[Trash Info]\nPath=/etc/passwd\n",
        )
        .unwrap();
        // Swapped the way another program would: the old `b` moved out of the trash (its inode
        // stays in use) and another, of another size, put in its place.
        std::fs::rename(&b.location, home.join("old-b.txt")).unwrap();
        std::fs::write(&b.location, "other").unwrap();
        for stale in [&a, &b] {
            let error = trash.delete_forever(stale).unwrap_err();
            assert!(super::super::is_changed_since_listed(&error), "{error}");
            assert!(stale.location.exists());
        }
        // An item that left the trash is gone, not changed.
        std::fs::remove_file(&b.location).unwrap();
        let error = trash.delete_forever(&b).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound, "{error}");
    }

    /// A `files/` or `info/` that is a link (to the user's documents, say) is never taken for the
    /// trash's own folder: nothing is listed, deleted, emptied or moved through it.
    #[test]
    fn a_trash_folder_that_is_a_link_is_not_used() {
        for linked in ["files", "info"] {
            let (_dir, trash, home) = fixture();
            trash_file(&trash, &home.join("a.txt"), "A");
            let [item] = trash.list().unwrap().try_into().unwrap();
            let documents = home.join("documents");
            std::fs::create_dir(&documents).unwrap();
            std::fs::write(documents.join("a.txt"), "mine").unwrap();
            std::fs::write(documents.join("a.txt.trashinfo"), "mine").unwrap();
            let folder = trash.root().join(linked);
            std::fs::rename(&folder, home.join("moved")).unwrap();
            std::os::unix::fs::symlink(&documents, &folder).unwrap();

            let mut linked_item = item.clone();
            linked_item.location = trash.files_dir().join("a.txt");
            let refusals = [
                trash.list().map(drop),
                trash.empty(),
                trash.delete_forever(&linked_item),
                trash.restore(&linked_item, &home.join("back.txt")),
            ];
            for result in refusals {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), ErrorKind::Unsupported, "{linked}: {error}");
            }
            std::fs::write(home.join("b.txt"), "B").unwrap();
            let error = trash
                .move_to_trash(&local(&home.join("b.txt")))
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Unsupported, "{linked}: {error}");
            assert!(home.join("b.txt").exists());
            assert_eq!(names(&documents), ["a.txt", "a.txt.trashinfo"], "{linked}");
            assert!(!home.join("back.txt").exists());
        }
    }

    /// A name too long to have a record (`<name>.trashinfo` over the file system's limit) is
    /// refused as the trash refuses an item, so that the caller offers a permanent delete.
    #[test]
    fn a_name_too_long_for_a_record_is_refused_as_unsupported() {
        let (_dir, trash, home) = fixture();
        let file = home.join(format!("{}.txt", "x".repeat(246)));
        std::fs::write(&file, "A").unwrap();
        let error = trash.move_to_trash(&local(&file)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unsupported, "{error}");
        assert!(file.exists());
        assert_eq!(names(&trash.info_dir()), Vec::<String>::new());
    }

    #[test]
    fn numbered_names_go_before_the_extension() {
        let n = |name: &str, n| numbered(OsStr::new(name), n).into_string().unwrap();
        assert_eq!(n("a.txt", 1), "a.txt");
        assert_eq!(n("a.txt", 2), "a.2.txt");
        assert_eq!(n("a.tar.gz", 3), "a.3.tar.gz");
        assert_eq!(n("a", 2), "a.2");
        assert_eq!(n(".bashrc", 2), ".bashrc.2");
        assert_eq!(n(".config.d", 2), ".config.2.d");
        assert_eq!(n("a.", 2), "a.2.");
    }

    /// `remove_info_record` takes a record only in the freedesktop layout.
    #[test]
    fn a_record_goes_only_from_beside_files() {
        let (_dir, trash, home) = fixture();
        let location = trash_file(&trash, &home.join("a.txt"), "A");
        std::fs::create_dir(home.join("info")).unwrap();
        std::fs::write(home.join("info").join("x.trashinfo"), "keep").unwrap();
        remove_info_record(&home.join("x"));
        assert!(home.join("info").join("x.trashinfo").exists());
        remove_info_record(&location);
        assert_eq!(names(&trash.info_dir()), Vec::<String>::new());
    }
}
