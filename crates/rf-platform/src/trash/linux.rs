//! The Linux home trash: `$XDG_DATA_HOME/Trash`, by default `~/.local/share/Trash`, kept as the
//! freedesktop.org Trash specification says ([`FreedesktopTrash`]), which file managers on Linux
//! share.
//!
//! It takes items on the file system of that folder only. The trashes other volumes keep at
//! their top (`.Trash-<uid>`) are not used: an item elsewhere is refused, and the caller asks
//! before a permanent delete, as on a USB stick on the other systems.
//!
//! An item in a temporary folder (the system's, `/tmp` and `/var/tmp`) is refused too, as if
//! that folder had no trash: those folders are emptied by the system on its own, and the refusal
//! keeps a test that deletes its temporary files with the trash on, in any crate, from ever
//! filling the user's own trash. Every function here takes its trash from [`home_trash`]; the
//! tests of the trash itself use a [`FreedesktopTrash`] in a temporary folder instead.

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

/// Whether `local` lies in a temporary folder: the one `std::env::temp_dir` names (as given and
/// with its links resolved), `/tmp` or `/var/tmp`.
fn in_temp_folder(local: &Path) -> bool {
    let temp = std::env::temp_dir();
    let resolved = temp.canonicalize().ok();
    let folders: [Option<PathBuf>; 4] = [
        Some(temp),
        resolved,
        Some(PathBuf::from("/tmp")),
        Some(PathBuf::from("/var/tmp")),
    ];
    folders
        .iter()
        .flatten()
        .any(|folder| local.starts_with(folder))
}

/// Whether the home trash takes the item at `local` (see the module documentation).
pub fn supported(local: &Path) -> bool {
    !in_temp_folder(local) && home_trash().is_some_and(|trash| trash.takes(local))
}

/// Moves `path` into the home trash; see [`FreedesktopTrash::move_to_trash`].
pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
    let trash = home_trash().ok_or_else(no_home_trash)?;
    if path.as_local().is_some_and(in_temp_folder) {
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
        assert!(in_temp_folder(&file));
        assert!(!supported(&file));
        let error = move_to_trash(&VPath::local(&file).unwrap()).unwrap_err();
        assert_eq!(error.kind(), rf_core::ErrorKind::Unsupported);
        assert!(file.exists());
        assert!(in_temp_folder(Path::new("/var/tmp/x")));
        assert!(!in_temp_folder(Path::new("/home/kim/tmp/x")));
    }
}
