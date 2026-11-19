//! The places of the sidebar: the user's well-known folders, the drives, and the places of the
//! shell, 내 PC and 휴지통 (NAV-16, [`shell_places`]).
//!
//! The first two lists ask the system (the shell's known folders on Windows, the file system
//! for the drives and their free space), which can block on a slow or sleeping drive, so the
//! functions here are called on a worker thread, never on the UI thread. The places of the
//! shell are known without a look.

use rf_core::VPath;
use rf_platform::{DiskSpace, KnownFolder, RootKind};
use rf_vfs::ShellPlace;

use crate::tree::TreeRowData;
use crate::words;

/// What kind of place a sidebar row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    /// One of the user's well-known folders, under "즐겨찾기", as long as the user has not
    /// edited the list.
    Favorite(KnownFolder),
    /// A folder the user keeps under "즐겨찾기" (NAV-05), once they have edited the list: added,
    /// dropped in, renamed or moved.
    Kept,
    /// A drive or mounted volume, under "이 PC".
    Drive,
    /// A place of the shell (NAV-16): 내 PC, first under "이 PC", or 휴지통, after the drives.
    /// It is a list, not a folder of the tree: it has no folders below it.
    Shell(ShellPlace),
}

impl PlaceKind {
    /// Whether the place is listed under "즐겨찾기": a well-known folder or one the user keeps.
    pub fn is_favorite(self) -> bool {
        matches!(self, PlaceKind::Favorite(_) | PlaceKind::Kept)
    }
}

/// 내 PC and 휴지통 as places of the sidebar: the first goes before the drives, the second after
/// them ([`AppState::set_places`](crate::state::AppState::set_places)).
pub fn shell_places() -> [Place; 2] {
    [ShellPlace::Computer, ShellPlace::Trash].map(|place| Place {
        label: words::shell_place(place).to_owned(),
        path: place.path(),
        kind: PlaceKind::Shell(place),
        space: None,
    })
}

/// The position among the `rows` of the sidebar's folder tree, whose roots are `places`, where
/// the group "최근 폴더" shows: right before the first row of a place that is no favorite (the
/// drives, under "이 PC"), else after the last row. The group is made of rows that are no folders
/// of the tree, so the tree's own rows keep their numbers and the window's list has the group's
/// rows between them.
pub fn recent_slot(rows: &[TreeRowData], places: &[Place]) -> usize {
    rows.iter()
        .position(|row| {
            row.root
                .and_then(|root| places.get(root))
                .is_some_and(|place| !place.kind.is_favorite())
        })
        .unwrap_or(rows.len())
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

/// The drives, each with its space from [`rf_platform::disk_space`], as
/// [`rf_platform::mounted_roots`] lists them (see there for the order and what is left out):
/// a Windows drive under [`words::drive_letter`], `/` under [`words::system_disk`], another
/// volume under the name of the folder it is mounted on. Blocks; call it on a worker thread.
pub fn drives() -> Vec<Place> {
    rf_platform::mounted_roots()
        .into_iter()
        .map(|root| {
            let space = rf_platform::disk_space(&root.path).ok();
            Place {
                label: drive_label(&root.kind),
                path: root.path,
                kind: PlaceKind::Drive,
                space,
            }
        })
        .collect()
}

/// The name the window gives a drive of `kind`.
pub fn drive_label(kind: &RootKind) -> String {
    match kind {
        RootKind::System => words::system_disk().to_owned(),
        RootKind::Letter(letter) => words::drive_letter(*letter),
        RootKind::Volume(name) => name.to_string_lossy().into_owned(),
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

    #[test]
    fn drives_are_named_by_their_kind() {
        assert_eq!(drive_label(&RootKind::System), "시스템 디스크");
        assert_eq!(drive_label(&RootKind::Letter('C')), "로컬 디스크 (C:)");
        assert_eq!(drive_label(&RootKind::Letter('D')), "드라이브 (D:)");
        assert_eq!(drive_label(&RootKind::Volume("USB".into())), "USB");
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
