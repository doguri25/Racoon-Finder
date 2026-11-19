//! The shell's places (NAV-16) as the provider of `Scheme::Shell`: `shell://trash`, what the
//! system trash holds, and `shell://computer`, the drives and mounted volumes with their space.
//!
//! Both are lists, not folders of the file system: nothing can be made, renamed, written, moved
//! or pasted in them. The trash takes two actions, putting an item back and deleting it for good
//! (and emptying the whole trash), which jobs run through [`Provider::restore_from_trash`],
//! [`Provider::delete_from_trash`] and [`Provider::empty_trash`]. A drive row is opened at its own
//! folder ([`Drive::root`]), which the local provider serves.
//!
//! An item's path is `shell://trash/<id>`, its `TrashId` as the one name; a drive's is
//! `shell://computer/<id>`, from its root. The listing gives an [`Entry`] for each, named by that
//! name, so that `dir.join(&entry.name)` is the item's path as for every provider; the columns of
//! a place ([`ShellPlace::columns`]) come from the typed rows ([`ShellProvider::trash_rows`],
//! [`ShellProvider::drive_rows`]), which a window lists on its worker as it lists any folder.
//!
//! The provider asks a [`TrashSource`] and a [`DriveSource`]: the system's own ([`SystemTrash`],
//! [`SystemDrives`]) in the app, others in tests.

use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use rf_core::{Entry, EntryKind, Error, Result, Scheme, VPath};
use rf_platform::{DiskSpace, RootKind, TrashItem};

use crate::cancel::CancelToken;
use crate::provider::{
    Capabilities, DeleteMode, Deleted, LIST_BATCH_SIZE, Provider, ReadStream, RenameMode,
    WriteMode, WriteStream,
};
use crate::watch::{WatchHandle, WatchSink};

/// One of the shell's places.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShellPlace {
    /// The system trash: `shell://trash`.
    Trash,
    /// The drives and mounted volumes: `shell://computer`.
    Computer,
}

/// What a column of a shell place shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShellColumn {
    Name,
    /// Where a trashed item came from (`TrashItem::original`); empty when the trash does not
    /// keep it.
    OriginalPlace,
    /// When an item was trashed (`TrashItem::deleted`).
    Deleted,
    /// An item's size (`TrashItem::size`), or a drive's (`DiskSpace::total`).
    Size,
    /// The space left on a drive for the user (`DiskSpace::available`).
    FreeSpace,
}

impl ShellPlace {
    /// Every place, in the order a sidebar shows them.
    pub const ALL: [ShellPlace; 2] = [ShellPlace::Computer, ShellPlace::Trash];

    /// The authority of the place's path: `trash` or `computer`.
    pub const fn name(self) -> &'static str {
        match self {
            ShellPlace::Trash => "trash",
            ShellPlace::Computer => "computer",
        }
    }

    /// The path of the place: `shell://trash` or `shell://computer`.
    pub fn path(self) -> VPath {
        VPath::new(Scheme::Shell, self.name(), &[]).expect("a place's name is one plain name")
    }

    /// The place `path` is or is in (`shell://trash/<id>` is in the trash), or `None` for
    /// another path.
    pub fn of(path: &VPath) -> Option<ShellPlace> {
        if path.scheme() != Scheme::Shell {
            return None;
        }
        let authority = path.authority()?;
        ShellPlace::ALL
            .into_iter()
            .find(|place| place.name() == authority)
    }

    /// The columns of the place's list, in the order a list shows them: for the trash its name,
    /// original place, the time it was trashed and its size; for the computer each drive's name,
    /// size and free space.
    pub const fn columns(self) -> &'static [ShellColumn] {
        match self {
            ShellPlace::Trash => &[
                ShellColumn::Name,
                ShellColumn::OriginalPlace,
                ShellColumn::Deleted,
                ShellColumn::Size,
            ],
            ShellPlace::Computer => &[ShellColumn::Name, ShellColumn::Size, ShellColumn::FreeSpace],
        }
    }

    /// What the place and its items allow: in the trash putting back and deleting for good only
    /// ([`Capabilities::RESTORE`], [`Capabilities::DELETE_FOREVER`]), with no
    /// [`Capabilities::WRITE`] (no paste, new folder, rename or moving in) and no trash of its
    /// own; nothing in the computer.
    pub const fn capabilities(self) -> Capabilities {
        match self {
            ShellPlace::Trash => Capabilities::RESTORE.union(Capabilities::DELETE_FOREVER),
            ShellPlace::Computer => Capabilities::empty(),
        }
    }
}

/// What the shell provider asks about the trash. The methods block; they run on a worker.
pub trait TrashSource: Send + Sync + fmt::Debug {
    /// Every item in the trash (see `rf_platform::trash::list`).
    fn list(&self) -> Result<Vec<TrashItem>>;
    /// Puts `item` back at `to`, never replacing anything (see `rf_platform::trash::restore`).
    fn restore(&self, item: &TrashItem, to: &Path) -> Result<()>;
    /// Deletes `item` for good (see `rf_platform::trash::delete_forever`).
    fn delete_forever(&self, item: &TrashItem) -> Result<()>;
    /// Deletes everything in the trash for good (see `rf_platform::trash::empty`).
    fn empty(&self) -> Result<()>;
    /// Whether the trash is emptied in one call that cannot be split into one per item (see
    /// `rf_platform::trash::EMPTIES_AT_ONCE`); the default implementation says no.
    fn empties_at_once(&self) -> bool {
        false
    }
}

/// The system trash of `rf_platform::trash`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTrash;

impl TrashSource for SystemTrash {
    fn list(&self) -> Result<Vec<TrashItem>> {
        rf_platform::trash::list()
    }

    fn restore(&self, item: &TrashItem, to: &Path) -> Result<()> {
        rf_platform::trash::restore(item, to)
    }

    fn delete_forever(&self, item: &TrashItem) -> Result<()> {
        rf_platform::trash::delete_forever(item)
    }

    fn empty(&self) -> Result<()> {
        rf_platform::trash::empty()
    }

    fn empties_at_once(&self) -> bool {
        rf_platform::trash::EMPTIES_AT_ONCE
    }
}

/// A freedesktop trash in a folder of its own, as tests keep one.
#[cfg(unix)]
impl TrashSource for rf_platform::FreedesktopTrash {
    fn list(&self) -> Result<Vec<TrashItem>> {
        rf_platform::FreedesktopTrash::list(self)
    }

    fn restore(&self, item: &TrashItem, to: &Path) -> Result<()> {
        rf_platform::FreedesktopTrash::restore(self, item, to)
    }

    fn delete_forever(&self, item: &TrashItem) -> Result<()> {
        rf_platform::FreedesktopTrash::delete_forever(self, item)
    }

    fn empty(&self) -> Result<()> {
        rf_platform::FreedesktopTrash::empty(self)
    }
}

/// A drive or mounted volume, as the computer lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// Its top folder, a local path: where a window goes when the row is opened.
    pub root: VPath,
    /// What it is, for the name a window gives it (`RootKind::Letter('C')` is "C:").
    pub kind: RootKind,
    /// Its size and free space, when the system tells.
    pub space: Option<DiskSpace>,
}

/// What the shell provider asks about the drives. It blocks on a slow or sleeping drive; it runs
/// on a worker.
pub trait DriveSource: Send + Sync + fmt::Debug {
    fn drives(&self) -> Vec<Drive>;
}

/// The system's drives: `rf_platform::mounted_roots`, each with its `rf_platform::disk_space`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemDrives;

impl DriveSource for SystemDrives {
    fn drives(&self) -> Vec<Drive> {
        rf_platform::mounted_roots()
            .into_iter()
            .map(|root| Drive {
                space: rf_platform::disk_space(&root.path).ok(),
                root: root.path,
                kind: root.kind,
            })
            .collect()
    }
}

/// One item of the trash with its path, `shell://trash/<id>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashRow {
    pub path: VPath,
    pub item: TrashItem,
}

/// One drive of the computer with its path, `shell://computer/<id>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveRow {
    pub path: VPath,
    pub drive: Drive,
}

/// The provider of `Scheme::Shell`. Cloning is cheap and shares the sources.
#[derive(Clone, Debug)]
pub struct ShellProvider {
    trash: Arc<dyn TrashSource>,
    drives: Arc<dyn DriveSource>,
}

impl Default for ShellProvider {
    fn default() -> Self {
        ShellProvider::new()
    }
}

impl ShellProvider {
    /// The places of this system: [`SystemTrash`] and [`SystemDrives`].
    pub fn new() -> ShellProvider {
        ShellProvider::with_sources(Arc::new(SystemTrash), Arc::new(SystemDrives))
    }

    /// The places as `trash` and `drives` tell them.
    pub fn with_sources(
        trash: Arc<dyn TrashSource>,
        drives: Arc<dyn DriveSource>,
    ) -> ShellProvider {
        ShellProvider { trash, drives }
    }

    /// Every item of the trash with its path. Blocks; call it on a worker.
    pub fn trash_rows(&self) -> Result<Vec<TrashRow>> {
        let trash = ShellPlace::Trash.path();
        self.trash
            .list()?
            .into_iter()
            .map(|item| {
                Ok(TrashRow {
                    path: trash.join(item.id.as_str())?,
                    item,
                })
            })
            .collect()
    }

    /// Every drive with its path. Blocks; call it on a worker.
    pub fn drive_rows(&self) -> Vec<DriveRow> {
        let computer = ShellPlace::Computer.path();
        self.drives
            .drives()
            .into_iter()
            .filter_map(|drive| {
                Some(DriveRow {
                    path: computer.join(drive_name(&drive.root)).ok()?,
                    drive,
                })
            })
            .collect()
    }

    /// The trash item `path` names (`shell://trash/<id>`), as the trash holds it now; `NotFound`
    /// when it holds no such item, and for any other path. Blocks; call it on a worker.
    pub fn trash_item(&self, path: &VPath) -> Result<TrashItem> {
        self.trash_rows()?
            .into_iter()
            .find(|row| row.path == *path)
            .map(|row| row.item)
            .ok_or_else(|| not_found("stat", path))
    }
}

/// The name a drive's row has (`shell://computer/<name>`): its root as text that holds no `/`,
/// as a `TrashId` is made.
pub fn drive_name(root: &VPath) -> String {
    root.to_uri().replace('/', "%2F")
}

/// The entry of a trash item, named by its id (`shell://trash/<id>`): a folder or a file of its
/// size, that shows the name it had (`Entry::label`), the folder it came from
/// (`Entry::origin`, when the trash keeps it) and when it was trashed (`Entry::deleted`). The
/// trash keeps no other time of the item.
pub fn trash_entry(item: &TrashItem) -> Entry {
    let kind = if item.is_dir {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    let mut entry = Entry::new(item.id.as_str(), kind);
    entry.size = item.size.unwrap_or(0);
    entry.label = Some(item.name.clone());
    entry.origin = item
        .original
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    entry.deleted = item.deleted;
    entry
}

/// The entry of a drive row: a folder named by its path's last name ([`drive_name`]), whose top
/// folder is its `Entry::origin`. The name a window shows for it is the window's to give
/// (from `Drive::kind`).
fn drive_entry(row: &DriveRow) -> Entry {
    let name = row.path.file_name().unwrap_or_default();
    let mut entry = Entry::new(name, EntryKind::Dir);
    entry.origin = row.drive.root.as_local().map(Path::to_path_buf);
    entry
}

/// `ErrorKind::NotFound` for `op` on `path`.
fn not_found(op: &'static str, path: &VPath) -> Error {
    Error::io(op, path.clone(), io::Error::from(io::ErrorKind::NotFound))
}

/// The refusal of `op` on `path`: the shell's places are lists, and none of their items can be
/// made, changed, read or moved as a file.
fn refused(op: &str, path: &VPath) -> Error {
    Error::Unsupported(format!("{op} is not possible in {path}"))
}

/// The place `path` is in, and the names after it.
fn split(path: &VPath) -> Option<(ShellPlace, &[String])> {
    Some((ShellPlace::of(path)?, path.segments()?))
}

/// Hands `entries` to `sink` in batches of at most [`LIST_BATCH_SIZE`], looking at `cancel`
/// before each.
fn send(entries: Vec<Entry>, cancel: &CancelToken, sink: &mut dyn FnMut(Vec<Entry>)) -> Result<()> {
    let mut entries = entries.into_iter().peekable();
    while entries.peek().is_some() {
        cancel.check()?;
        sink(entries.by_ref().take(LIST_BATCH_SIZE).collect());
    }
    Ok(())
}

impl Provider for ShellProvider {
    fn scheme(&self) -> Scheme {
        Scheme::Shell
    }

    fn capabilities(&self, path: &VPath) -> Capabilities {
        ShellPlace::of(path).map_or(Capabilities::empty(), ShellPlace::capabilities)
    }

    /// Lists the trash or the computer. A trash item or a drive cannot be listed here: a drive is
    /// listed at its own root, which the local provider serves.
    fn list(
        &self,
        dir: &VPath,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Vec<Entry>),
    ) -> Result<()> {
        cancel.check()?;
        let entries = match split(dir) {
            Some((ShellPlace::Trash, [])) => self
                .trash_rows()?
                .iter()
                .map(|row| trash_entry(&row.item))
                .collect(),
            Some((ShellPlace::Computer, [])) => self.drive_rows().iter().map(drive_entry).collect(),
            Some((_, [_, ..])) => return Err(refused("list", dir)),
            None => return Err(not_found("list", dir)),
        };
        send(entries, cancel, sink)
    }

    fn stat(&self, path: &VPath) -> Result<Entry> {
        match split(path) {
            Some((place, [])) => Ok(Entry::new(place.name(), EntryKind::Dir)),
            Some((ShellPlace::Trash, [_])) => Ok(trash_entry(&self.trash_item(path)?)),
            Some((ShellPlace::Computer, [_])) => self
                .drive_rows()
                .iter()
                .find(|row| row.path == *path)
                .map(drive_entry)
                .ok_or_else(|| not_found("stat", path)),
            _ => Err(not_found("stat", path)),
        }
    }

    fn create_dir(&self, path: &VPath) -> Result<()> {
        Err(refused("create_dir", path))
    }

    fn create_file(&self, path: &VPath) -> Result<()> {
        Err(refused("create_file", path))
    }

    fn rename(&self, from: &VPath, _to: &VPath, _mode: RenameMode) -> Result<()> {
        Err(refused("rename", from))
    }

    /// Refused: an item of the trash is deleted for good with [`Provider::delete_from_trash`],
    /// and a drive is no item to delete.
    fn delete(&self, path: &VPath, _mode: DeleteMode) -> Result<Deleted> {
        Err(refused("delete", path))
    }

    fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
        Err(refused("open_read", path))
    }

    fn open_write(&self, path: &VPath, _mode: WriteMode) -> Result<Box<dyn WriteStream>> {
        Err(refused("open_write", path))
    }

    fn space(&self, _path: &VPath) -> Result<Option<DiskSpace>> {
        Ok(None)
    }

    fn watch(&self, dir: &VPath, _sink: WatchSink) -> Result<WatchHandle> {
        Err(refused("watch", dir))
    }

    /// Refused: nothing can be removed from a shell place this way.
    fn delete_leaf(&self, path: &VPath) -> Result<()> {
        Err(refused("delete_leaf", path))
    }

    fn restore_from_trash(&self, item: &TrashItem, to: &VPath) -> Result<()> {
        let Some(local) = to.as_local() else {
            return Err(Error::Unsupported(format!(
                "an item comes out of the trash to a local place only: {to}"
            )));
        };
        self.trash.restore(item, local)
    }

    fn delete_from_trash(&self, item: &TrashItem) -> Result<()> {
        self.trash.delete_forever(item)
    }

    fn trash_items(&self) -> Result<Vec<TrashItem>> {
        self.trash.list()
    }

    fn empties_trash_at_once(&self) -> bool {
        self.trash.empties_at_once()
    }

    fn empty_trash(&self) -> Result<()> {
        self.trash.empty()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime};

    use rf_core::ErrorKind;

    use super::*;

    /// What a [`FakeTrash`] was asked to do.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Asked {
        Restore(PathBuf, PathBuf),
        DeleteForever(PathBuf),
        Empty,
    }

    /// A trash that lists the items it was given and notes what it is asked.
    #[derive(Debug, Default)]
    struct FakeTrash {
        items: Vec<TrashItem>,
        asked: Mutex<Vec<Asked>>,
    }

    impl FakeTrash {
        fn note(&self, asked: Asked) {
            self.asked.lock().unwrap().push(asked);
        }
    }

    impl TrashSource for FakeTrash {
        fn list(&self) -> Result<Vec<TrashItem>> {
            Ok(self.items.clone())
        }
        fn restore(&self, item: &TrashItem, to: &Path) -> Result<()> {
            self.note(Asked::Restore(item.location.clone(), to.to_path_buf()));
            Ok(())
        }
        fn delete_forever(&self, item: &TrashItem) -> Result<()> {
            self.note(Asked::DeleteForever(item.location.clone()));
            Ok(())
        }
        fn empty(&self) -> Result<()> {
            self.note(Asked::Empty);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FakeDrives(Vec<Drive>);

    impl DriveSource for FakeDrives {
        fn drives(&self) -> Vec<Drive> {
            self.0.clone()
        }
    }

    fn base() -> PathBuf {
        std::env::temp_dir()
    }

    fn item(name: &str, is_dir: bool) -> TrashItem {
        let mut item = TrashItem::at(base().join("bin").join(name), is_dir);
        item.original = Some(base().join("home").join(name));
        item.deleted = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        item.size = (!is_dir).then_some(42);
        item
    }

    fn drive(name: &str, kind: RootKind) -> Drive {
        Drive {
            root: VPath::local(base().join(name)).unwrap(),
            kind,
            space: Some(DiskSpace {
                total: 100,
                free: 60,
                available: 50,
            }),
        }
    }

    fn provider(items: Vec<TrashItem>) -> (Arc<FakeTrash>, ShellProvider) {
        let trash = Arc::new(FakeTrash {
            items,
            ..FakeTrash::default()
        });
        let drives = FakeDrives(vec![
            drive("system", RootKind::System),
            drive("usb", RootKind::Volume("USB".into())),
        ]);
        let shell = ShellProvider::with_sources(trash.clone(), Arc::new(drives));
        (trash, shell)
    }

    fn listed(shell: &ShellProvider, dir: &VPath) -> Result<Vec<Entry>> {
        let mut all = Vec::new();
        shell.list(dir, &CancelToken::new(), &mut |batch| {
            assert!(!batch.is_empty() && batch.len() <= LIST_BATCH_SIZE);
            all.extend(batch);
        })?;
        Ok(all)
    }

    #[test]
    fn places_have_their_paths_and_columns() {
        assert_eq!(ShellPlace::Trash.path().to_uri(), "shell://trash");
        assert_eq!(ShellPlace::Computer.path().to_uri(), "shell://computer");
        for place in ShellPlace::ALL {
            assert_eq!(ShellPlace::of(&place.path()), Some(place));
            assert_eq!(
                ShellPlace::of(&place.path().join("x").unwrap()),
                Some(place)
            );
        }
        assert_eq!(ShellPlace::of(&VPath::local(base()).unwrap()), None);
        let other = VPath::new(Scheme::Shell, "network", &[]).unwrap();
        assert_eq!(ShellPlace::of(&other), None);
        assert_eq!(
            ShellPlace::Trash.columns(),
            [
                ShellColumn::Name,
                ShellColumn::OriginalPlace,
                ShellColumn::Deleted,
                ShellColumn::Size
            ]
        );
        assert_eq!(
            ShellPlace::Computer.columns(),
            [ShellColumn::Name, ShellColumn::Size, ShellColumn::FreeSpace]
        );
    }

    #[test]
    fn the_trash_allows_putting_back_and_deleting_for_good_only() {
        let (_, shell) = provider(vec![item("a.txt", false)]);
        let trash = ShellPlace::Trash.path();
        let row = &shell.trash_rows().unwrap()[0];
        for path in [&trash, &row.path] {
            let caps = shell.capabilities(path);
            assert_eq!(caps, Capabilities::RESTORE | Capabilities::DELETE_FOREVER);
            assert!(!caps.intersects(Capabilities::WRITE | Capabilities::TRASH));
        }
        let computer = ShellPlace::Computer.path();
        assert_eq!(shell.capabilities(&computer), Capabilities::empty());
        // Nothing is made, renamed, written, deleted or moved in.
        let new = trash.join("new").unwrap();
        let refusals = [
            shell.create_dir(&new).unwrap_err(),
            shell.create_file(&new).unwrap_err(),
            shell
                .rename(&row.path, &new, RenameMode::NoReplace)
                .unwrap_err(),
            shell.delete(&row.path, DeleteMode::Permanent).unwrap_err(),
            shell.delete(&row.path, DeleteMode::Trash).unwrap_err(),
            shell.delete_leaf(&row.path).unwrap_err(),
            shell.open_write(&new, WriteMode::CreateNew).unwrap_err(),
            shell.open_read(&row.path).err().unwrap(),
            shell.watch(&trash, Box::new(|_| {})).err().unwrap(),
        ];
        for error in refusals {
            assert_eq!(error.kind(), ErrorKind::Unsupported, "{error}");
        }
        assert_eq!(shell.space(&trash).unwrap(), None);
    }

    #[test]
    fn the_trash_lists_its_items_by_id() {
        let items = vec![item("a.txt", false), item("folder", true)];
        let (_, shell) = provider(items.clone());
        let trash = ShellPlace::Trash.path();
        let entries = listed(&shell, &trash).unwrap();
        assert_eq!(entries.len(), 2);
        for (entry, item) in entries.iter().zip(&items) {
            assert_eq!(entry.name.to_str(), Some(item.id.as_str()));
            let path = trash.join(&entry.name).unwrap();
            assert_eq!(shell.trash_item(&path).unwrap(), *item);
            assert_eq!(shell.stat(&path).unwrap(), *entry);
            // It shows the name it had, from the folder it came from, and when it was trashed.
            assert_eq!(entry.shown_name(), item.name.as_os_str());
            assert_eq!(
                entry.origin.as_deref(),
                item.original.as_deref().and_then(Path::parent)
            );
            assert_eq!((entry.deleted, entry.modified), (item.deleted, None));
        }
        assert_eq!((entries[0].kind, entries[0].size), (EntryKind::File, 42));
        assert_eq!((entries[1].kind, entries[1].size), (EntryKind::Dir, 0));
        let rows = shell.trash_rows().unwrap();
        assert_eq!(
            rows.iter().map(|r| &r.item).collect::<Vec<_>>(),
            items.iter().collect::<Vec<_>>()
        );
        // A gone item, a path below an item and another place are not there.
        let gone = trash.join("gone").unwrap();
        assert_eq!(shell.stat(&gone).unwrap_err().kind(), ErrorKind::NotFound);
        let below = rows[1].path.join("x").unwrap();
        assert_eq!(shell.stat(&below).unwrap_err().kind(), ErrorKind::NotFound);
        assert_eq!(
            listed(&shell, &rows[1].path).unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        let other = VPath::new(Scheme::Shell, "network", &[]).unwrap();
        assert_eq!(
            listed(&shell, &other).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert_eq!(shell.stat(&trash).unwrap().kind, EntryKind::Dir);
    }

    #[test]
    fn a_long_trash_comes_in_batches_and_stops_when_cancelled() {
        let items: Vec<TrashItem> = (0..=LIST_BATCH_SIZE)
            .map(|n| item(&format!("{n}.txt"), false))
            .collect();
        let (_, shell) = provider(items);
        let mut batches = Vec::new();
        let trash = ShellPlace::Trash.path();
        shell
            .list(&trash, &CancelToken::new(), &mut |batch| {
                batches.push(batch.len());
            })
            .unwrap();
        assert_eq!(batches, [LIST_BATCH_SIZE, 1]);
        let cancel = CancelToken::new();
        let mut calls = 0;
        let stopped = shell.list(&trash, &cancel, &mut |_| {
            calls += 1;
            cancel.cancel();
        });
        assert_eq!(stopped.unwrap_err().kind(), ErrorKind::Cancelled);
        assert_eq!(calls, 1);
    }

    #[test]
    fn the_trash_actions_go_to_the_trash() {
        let a = item("a.txt", false);
        let (trash, shell) = provider(vec![a.clone()]);
        let to = VPath::local(base().join("back.txt")).unwrap();
        shell.restore_from_trash(&a, &to).unwrap();
        shell.delete_from_trash(&a).unwrap();
        shell.empty_trash().unwrap();
        let remote = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
        assert_eq!(
            shell.restore_from_trash(&a, &remote).unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        assert_eq!(
            *trash.asked.lock().unwrap(),
            [
                Asked::Restore(a.location.clone(), base().join("back.txt")),
                Asked::DeleteForever(a.location.clone()),
                Asked::Empty
            ]
        );
    }

    #[test]
    fn the_computer_lists_its_drives() {
        let (_, shell) = provider(Vec::new());
        let computer = ShellPlace::Computer.path();
        let rows = shell.drive_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].drive.kind, RootKind::Volume("USB".into()));
        assert_eq!(rows[1].drive.space.unwrap().available, 50);
        let entries = listed(&shell, &computer).unwrap();
        for (entry, row) in entries.iter().zip(&rows) {
            assert_eq!(computer.join(&entry.name).unwrap(), row.path);
            assert_eq!(entry.kind, EntryKind::Dir);
            assert_eq!(entry.origin.as_deref(), row.drive.root.as_local());
            assert_eq!(
                entry.name.to_str(),
                Some(drive_name(&row.drive.root).as_str())
            );
            assert_eq!(shell.stat(&row.path).unwrap(), *entry);
            // Opened at its own root, not here.
            assert_eq!(
                listed(&shell, &row.path).unwrap_err().kind(),
                ErrorKind::Unsupported
            );
        }
        assert_ne!(rows[0].path, rows[1].path);
        let gone = computer.join("gone").unwrap();
        assert_eq!(shell.stat(&gone).unwrap_err().kind(), ErrorKind::NotFound);
    }

    /// The system's drives answer on every OS. (The system trash is not read here: a test never
    /// touches the user's own trash.)
    #[test]
    fn the_system_drives_can_be_listed() {
        let rows = SystemDrives.drives();
        if cfg!(unix) {
            assert_eq!(rows[0].kind, RootKind::System);
        }
        assert!(rows.iter().all(|drive| drive.root.as_local().is_some()));
    }
}
