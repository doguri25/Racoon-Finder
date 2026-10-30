//! The places of the sidebar: the user's well-known folders and the drives.
//!
//! Both lists ask the system (the shell's known folders on Windows, the file system for the
//! drives and their free space), which can block on a slow or sleeping drive, so the functions
//! here are called on a worker thread, never on the UI thread.

use rf_core::VPath;
use rf_platform::{DiskSpace, KnownFolder};

use crate::words;

/// What kind of place a sidebar row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    /// One of the user's well-known folders, under "즐겨찾기".
    Favorite(KnownFolder),
    /// A drive or mounted volume, under "이 PC".
    Drive,
}

/// One row of the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The name the row shows.
    pub label: String,
    /// The folder the row opens.
    pub path: VPath,
    pub kind: PlaceKind,
    /// The size and free space of a drive, when the system tells; `None` for a favorite.
    pub space: Option<DiskSpace>,
}

/// The user's well-known folders that the system defines, in the order of
/// [`KnownFolder::ALL`], under their [`words::favorite`] names. Blocks; call it on a worker
/// thread.
pub fn favorites() -> Vec<Place> {
    KnownFolder::ALL
        .into_iter()
        .filter_map(|folder| {
            Some(Place {
                label: words::favorite(folder).to_owned(),
                path: rf_platform::known_folder(folder)?,
                kind: PlaceKind::Favorite(folder),
                space: None,
            })
        })
        .collect()
}

/// The drives, each with its space from [`rf_platform::disk_space`]. Blocks; call it on a
/// worker thread.
///
/// - Windows: every drive letter whose root can be read, named by [`words::drive_letter`].
/// - macOS: `/` as [`words::system_disk`], then the folders of `/Volumes` that are other
///   volumes than `/` (which skips the link to the system volume there).
/// - Linux: `/` as [`words::system_disk`], then the folders of `/media/*`, `/run/media/*` and
///   `/mnt` that are other volumes than `/`, each volume once, under its folder's name.
///
/// Folders whose names start with a dot (`/Volumes/.timemachine`) are the system's own and
/// are left out.
pub fn drives() -> Vec<Place> {
    imp::drive_roots()
        .into_iter()
        .map(|(label, path)| {
            let space = rf_platform::disk_space(&path).ok();
            Place {
                label,
                path,
                kind: PlaceKind::Drive,
                space,
            }
        })
        .collect()
}

#[cfg(windows)]
mod imp {
    use rf_core::VPath;

    use crate::words;

    /// The readable drive letters, as labels and roots.
    pub(super) fn drive_roots() -> Vec<(String, VPath)> {
        ('A'..='Z')
            .filter_map(|letter| {
                let root = format!("{letter}:\\");
                std::fs::metadata(&root).ok()?;
                Some((words::drive_letter(letter), VPath::local(&root).ok()?))
            })
            .collect()
    }
}

#[cfg(unix)]
mod imp {
    use std::collections::HashSet;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use rf_core::VPath;

    use crate::words;

    /// The folders that may hold other volumes than `/`, as globs of one or two levels.
    #[cfg(target_os = "macos")]
    const MOUNT_FOLDERS: &[(&str, usize)] = &[("/Volumes", 1)];
    #[cfg(not(target_os = "macos"))]
    const MOUNT_FOLDERS: &[(&str, usize)] = &[("/media", 2), ("/run/media", 2), ("/mnt", 1)];

    /// `/` and the volumes mounted on the folders of [`MOUNT_FOLDERS`], as labels and roots.
    pub(super) fn drive_roots() -> Vec<(String, VPath)> {
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        let root = Path::new("/");
        if let Ok(meta) = std::fs::metadata(root) {
            seen.insert(meta.dev());
        }
        if let Ok(path) = VPath::local(root) {
            roots.push((words::system_disk().to_owned(), path));
        }
        for &(base, depth) in MOUNT_FOLDERS {
            for folder in folders_at(Path::new(base), depth) {
                // A folder on the device of a volume already listed is no mount of its own;
                // the device also tells a volume reached under two paths.
                let Ok(meta) = std::fs::metadata(&folder) else {
                    continue;
                };
                if !meta.is_dir() || !seen.insert(meta.dev()) {
                    continue;
                }
                let label = folder
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                if let Ok(path) = VPath::local(&folder) {
                    roots.push((label, path));
                }
            }
        }
        roots
    }

    /// The folders `depth` levels below `base`, sorted by path, without those whose names
    /// start with a dot.
    pub(super) fn folders_at(base: &Path, depth: usize) -> Vec<PathBuf> {
        let mut level = vec![base.to_path_buf()];
        for _ in 0..depth {
            level = level
                .iter()
                .filter_map(|folder| std::fs::read_dir(folder).ok())
                .flatten()
                .filter_map(Result::ok)
                .filter(|child| !child.file_name().as_encoded_bytes().starts_with(b"."))
                .map(|child| child.path())
                .collect();
        }
        level.sort();
        level
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use rf_core::VPath;

    pub(super) fn drive_roots() -> Vec<(String, VPath)> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_are_in_sidebar_order() {
        let labels = [
            (KnownFolder::Home, "홈"),
            (KnownFolder::Desktop, "바탕 화면"),
            (KnownFolder::Documents, "문서"),
            (KnownFolder::Downloads, "다운로드"),
            (KnownFolder::Pictures, "사진"),
            (KnownFolder::Music, "음악"),
            (KnownFolder::Videos, "동영상"),
        ];
        let expected: Vec<Place> = labels
            .iter()
            .filter_map(|&(folder, label)| {
                Some(Place {
                    label: label.to_owned(),
                    path: rf_platform::known_folder(folder)?,
                    kind: PlaceKind::Favorite(folder),
                    space: None,
                })
            })
            .collect();
        assert_eq!(favorites(), expected);
        // Every test machine has a home folder, and it comes first.
        assert_eq!(expected[0].kind, PlaceKind::Favorite(KnownFolder::Home));
    }

    #[cfg(unix)]
    #[test]
    fn mount_folders_skip_dot_folders() {
        let t = tempfile::tempdir().expect("a temporary folder");
        for name in ["user/USB", "user/.hidden", ".timemachine/x", "Data"] {
            std::fs::create_dir_all(t.path().join(name)).expect("a folder");
        }
        assert_eq!(
            imp::folders_at(t.path(), 1),
            [t.path().join("Data"), t.path().join("user")]
        );
        assert_eq!(imp::folders_at(t.path(), 2), [t.path().join("user/USB")]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_drives_include_root() {
        let drives = drives();
        let root = &drives[0];
        assert_eq!(
            root.path,
            VPath::local("/").expect("the root is a local path")
        );
        assert_eq!(root.label, "시스템 디스크");
        assert_eq!(root.kind, PlaceKind::Drive);
        let space = root.space.expect("the root's space is known");
        assert!(space.total > 0 && space.available <= space.total);
        assert!(drives.iter().all(|drive| drive.kind == PlaceKind::Drive));
        // Each volume is listed once.
        for (i, drive) in drives.iter().enumerate() {
            assert!(
                drives[..i].iter().all(|other| other.path != drive.path),
                "{drives:?}"
            );
        }
    }
}
