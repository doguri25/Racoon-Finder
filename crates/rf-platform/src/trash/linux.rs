//! The Linux home trash: `$XDG_DATA_HOME/Trash`, by default `~/.local/share/Trash`, kept as the
//! freedesktop.org Trash specification says ([`FreedesktopTrash`]), which file managers on Linux
//! share.
//!
//! It takes items on the file system of that folder only. The trashes other volumes keep at
//! their top (`.Trash-<uid>`) are not used: an item elsewhere is refused, and the caller asks
//! before a permanent delete, as on a USB stick on the other systems.
//!
//! An item in a temporary folder is refused too, as if that folder had no trash: `/tmp`,
//! `/var/tmp` and the folder `std::env::temp_dir` names (`$TMPDIR`), as given and with its links
//! resolved. Those folders are emptied by the system on its own, and the refusal keeps a test
//! that deletes its temporary files with the trash on, in any crate, from ever filling the user's
//! own trash. A temporary folder that is the home folder or holds the trash (`TMPDIR=$HOME`) does
//! not count: it would turn the trash off for the whole home. So on Linux deleting an item of
//! `/tmp` or `/var/tmp` asks for a permanent delete, as on a USB stick, even where those folders
//! share the home's file system (GNOME and KDE would trash it).
//!
//! Every function here takes its trash from [`home_trash`]; the tests of the trash itself use a
//! [`FreedesktopTrash`] in a temporary folder instead.

use std::path::{Path, PathBuf};

use rf_core::{Error, Result, VPath};

use super::{FreedesktopTrash, TrashItem};

/// The home trash, or `None` when the system names no data folder for the user (no `HOME`).
fn home_trash() -> Option<FreedesktopTrash> {
    dirs::data_dir().map(|data| FreedesktopTrash::new(data.join("Trash")))
}

/// The error of a system without a home trash.
fn no_home_trash() -> Error {
    Error::Unsupported("no home folder, so no trash; delete permanently instead".to_owned())
}

/// The temporary folders whose items the home trash at `trash` refuses: `/tmp`, `/var/tmp` and
/// `temp` (`std::env::temp_dir`), each as given and with its links resolved, except one that is
/// `home` or a folder that holds the trash.
fn temp_folders(temp: &Path, home: Option<&Path>, trash: &Path) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    for folder in [temp, Path::new("/tmp"), Path::new("/var/tmp")] {
        let resolved = folder.canonicalize().ok();
        for folder in [Some(folder.to_path_buf()), resolved].into_iter().flatten() {
            let holds_home = home.is_some_and(|home| folder == home);
            if !holds_home && !trash.starts_with(&folder) && !folders.contains(&folder) {
                folders.push(folder);
            }
        }
    }
    folders
}

/// Whether `local` lies in one of the temporary folders of the home trash at `trash` (see
/// [`temp_folders`]).
fn in_temp_folder(local: &Path, trash: &Path) -> bool {
    let home = dirs::home_dir();
    temp_folders(&std::env::temp_dir(), home.as_deref(), trash)
        .iter()
        .any(|folder| local.starts_with(folder))
}

/// Whether the home trash takes the item at `local` (see the module documentation).
pub fn supported(local: &Path) -> bool {
    home_trash().is_some_and(|trash| !in_temp_folder(local, trash.root()) && trash.takes(local))
}

/// Moves `path` into the home trash; see [`FreedesktopTrash::move_to_trash`].
pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
    let trash = home_trash().ok_or_else(no_home_trash)?;
    if path
        .as_local()
        .is_some_and(|local| in_temp_folder(local, trash.root()))
    {
        return Err(Error::Unsupported(format!(
            "a temporary folder has no trash: {path}; delete permanently instead"
        )));
    }
    trash.move_to_trash(path)
}

/// The items of the home trash; none when there is no home.
pub fn list() -> Result<Vec<TrashItem>> {
    home_trash().map_or_else(|| Ok(Vec::new()), |trash| trash.list())
}

pub fn restore(item: &TrashItem, to: &Path) -> Result<()> {
    home_trash().ok_or_else(no_home_trash)?.restore(item, to)
}

pub fn delete_forever(item: &TrashItem) -> Result<()> {
    home_trash().ok_or_else(no_home_trash)?.delete_forever(item)
}

pub fn empty() -> Result<()> {
    home_trash().map_or(Ok(()), |trash| trash.empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever the home trash is, a test's temporary files never go there.
    #[test]
    fn temporary_folders_have_no_trash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "A").unwrap();
        let trash = home_trash().expect("a home folder");
        assert!(in_temp_folder(&file, trash.root()));
        assert!(!supported(&file));
        let error = move_to_trash(&VPath::local(&file).unwrap()).unwrap_err();
        assert_eq!(error.kind(), rf_core::ErrorKind::Unsupported);
        assert!(file.exists());
    }

    /// `TMPDIR` pointing at the home folder, or at a folder that holds the trash, does not turn
    /// the trash off there; `/tmp`, `/var/tmp` and any other `TMPDIR` stay without one.
    #[test]
    fn a_temporary_folder_that_holds_the_home_does_not_count() {
        let home = Path::new("/home/kim");
        let trash = Path::new("/home/kim/.local/share/Trash");
        let fixed = [PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];
        let only_fixed = |temp: &str| {
            let folders = temp_folders(Path::new(temp), Some(home), trash);
            fixed.iter().all(|f| folders.contains(f))
                && !folders
                    .iter()
                    .any(|f| f.starts_with(home) || home.starts_with(f))
        };
        assert!(only_fixed("/home/kim"));
        assert!(only_fixed("/home/kim/.local"));
        assert!(only_fixed("/home"));
        let own = temp_folders(Path::new("/home/kim/tmp"), Some(home), trash);
        assert!(own.contains(&PathBuf::from("/home/kim/tmp")), "{own:?}");
        let run = temp_folders(Path::new("/run/user/1000/tmp"), Some(home), trash);
        assert!(
            run.contains(&PathBuf::from("/run/user/1000/tmp")),
            "{run:?}"
        );
        // A trash kept in /tmp (an odd `XDG_DATA_HOME`) keeps /tmp's items.
        let in_tmp = temp_folders(Path::new("/tmp"), Some(home), Path::new("/tmp/data/Trash"));
        assert!(!in_tmp.contains(&PathBuf::from("/tmp")), "{in_tmp:?}");
    }
}
