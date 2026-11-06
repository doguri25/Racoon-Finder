//! The app bundle of the running app (macOS, PRD INST-05): where it is, and moving it to the
//! trash as the last step of "Racoon Finder 완전 삭제…".
//!
//! A bundle is a folder `<Name>.app` with the executable at `Contents/MacOS/<executable>`
//! ([`app_bundle_of`]); only an executable in exactly that place counts, so a program run from a
//! build folder (`cargo run`) or from a folder that merely has an `.app` somewhere above it is no
//! bundle, and nothing is moved. The bundle goes to the trash through `NSFileManager`, which
//! moves the folder whole (the app that runs from it goes on until it ends).
//!
//! [`move_app_bundle_to_trash`] blocks (a disk, perhaps a slow one): call it from a worker
//! thread, never from the UI thread.

use std::io;
use std::path::{Path, PathBuf};

/// The `.app` folder an executable at `exe` is in: `exe` must be at
/// `<Name>.app/Contents/MacOS/<executable>`. `None` for anything else, a path that only has an
/// `.app` folder somewhere above it among them. Pure: the path is not looked at on disk.
pub fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |path: &Path, name: &str| path.file_name().is_some_and(|found| found == name);
    let is_app = bundle
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        && bundle.file_stem().is_some_and(|stem| !stem.is_empty());
    (named(macos, "MacOS") && named(contents, "Contents") && is_app).then(|| bundle.to_path_buf())
}

/// Why the app bundle of the running app cannot be moved to the trash now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrashBlock {
    /// The executable is in no app bundle (a build folder, a portable folder).
    NotInBundle,
    /// Gatekeeper runs the app from a read-only copy (App Translocation): the real bundle is
    /// somewhere else, and the one that runs cannot be moved.
    Translocated,
    /// The bundle is on a volume mounted read-only (a mounted disk image).
    ReadOnlyVolume,
    /// The folder the bundle is in cannot be written by this user, which moving it out needs.
    FolderNotWritable,
}

/// The block a bundle at `bundle` has by its path alone: Gatekeeper's translocated copies are
/// under a folder `AppTranslocation`. Pure.
pub fn trash_block_by_path(bundle: &Path) -> Option<TrashBlock> {
    bundle
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
        .then_some(TrashBlock::Translocated)
}

/// Whether the app bundle `exe` is in can be moved to the trash, as far as the system tells
/// before anything is tried: it must be a bundle ([`app_bundle_of`]), not a translocated copy,
/// not on a read-only volume, and in a folder this user can write. Reads the disk (a stat of
/// the bundle's folder): call it from a worker thread. `Ok` carries the bundle, by the path the
/// trash will be given.
pub fn can_trash_app_bundle(exe: &Path) -> Result<PathBuf, TrashBlock> {
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let bundle = app_bundle_of(&exe).ok_or(TrashBlock::NotInBundle)?;
    if let Some(block) = trash_block_by_path(&bundle) {
        return Err(block);
    }
    let parent = bundle.parent().ok_or(TrashBlock::FolderNotWritable)?;
    if volume_is_read_only(&bundle) {
        return Err(TrashBlock::ReadOnlyVolume);
    }
    if !folder_is_writable(parent) {
        return Err(TrashBlock::FolderNotWritable);
    }
    Ok(bundle)
}

#[cfg(unix)]
fn volume_is_read_only(path: &Path) -> bool {
    use rustix::fs::{StatVfsMountFlags, statvfs};
    statvfs(path).is_ok_and(|stat| stat.f_flag.contains(StatVfsMountFlags::RDONLY))
}

#[cfg(not(unix))]
fn volume_is_read_only(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
fn folder_is_writable(folder: &Path) -> bool {
    use rustix::fs::AtFlags;
    use rustix::fs::{Access, CWD, accessat};
    accessat(
        CWD,
        folder,
        Access::WRITE_OK | Access::EXEC_OK,
        AtFlags::empty(),
    )
    .is_ok()
}

#[cfg(not(unix))]
fn folder_is_writable(folder: &Path) -> bool {
    std::fs::metadata(folder).is_ok_and(|meta| !meta.permissions().readonly())
}

/// Moves the app bundle the running executable is in to the trash, and returns where it went
/// (the bundle's own place when the system did not say). Fails, and moves nothing, when the
/// system is not macOS (`Unsupported`) or the executable is not in an app bundle
/// (`InvalidInput`), and with the system's reason when the trash refuses the folder (an app
/// installed by an administrator, say). Blocks.
pub fn move_app_bundle_to_trash() -> io::Result<PathBuf> {
    if !cfg!(target_os = "macos") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "only a macOS app bundle is moved to the trash",
        ));
    }
    let exe = std::env::current_exe()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let bundle = app_bundle_of(&exe).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the program does not run from an app bundle",
        )
    })?;
    trash(&bundle)
}

#[cfg(target_os = "macos")]
fn trash(bundle: &Path) -> io::Result<PathBuf> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSFileManager, NSString, NSURL};

    let text = bundle.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path is not valid UTF-8, which the trash API needs",
        )
    })?;
    let moved = autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));
        let mut resulting = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
            .map_err(|error| io::Error::other(error.localizedDescription().to_string()))?;
        Ok::<_, io::Error>(
            resulting
                .and_then(|moved| moved.path())
                .map(|moved| PathBuf::from(moved.to_string())),
        )
    })?;
    // Whatever the system reported, the bundle must be gone from its place.
    if std::fs::symlink_metadata(bundle).is_ok() {
        return Err(io::Error::other(
            "the trash reported success but the app is still there",
        ));
    }
    Ok(moved.unwrap_or_else(|| bundle.to_path_buf()))
}

#[cfg(not(target_os = "macos"))]
fn trash(_bundle: &Path) -> io::Result<PathBuf> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_found_from_an_executable_in_its_place() {
        let exe = Path::new("/Applications/Racoon Finder.app/Contents/MacOS/racoon-finder");
        assert_eq!(
            app_bundle_of(exe),
            Some(PathBuf::from("/Applications/Racoon Finder.app"))
        );
        // Wherever the bundle is, and however its extension is cased.
        assert_eq!(
            app_bundle_of(Path::new("/Users/a/Downloads/Racoon.APP/Contents/MacOS/x")),
            Some(PathBuf::from("/Users/a/Downloads/Racoon.APP"))
        );
    }

    #[test]
    fn a_program_that_is_not_in_a_bundle_is_refused() {
        for odd in [
            // A build folder, a portable folder.
            "/home/a/Racoon-Finder/target/debug/racoon-finder",
            "/opt/racoon/racoon-finder",
            // An `.app` somewhere above, but not in the layout of a bundle.
            "/Applications/Racoon Finder.app/racoon-finder",
            "/Applications/Racoon Finder.app/Contents/racoon-finder",
            "/Applications/Racoon Finder.app/Contents/Resources/racoon-finder",
            "/Applications/Racoon Finder.app/Contents/MacOS/bin/racoon-finder",
            // The folders of the layout under a folder that is no `.app`.
            "/Applications/Racoon Finder/Contents/MacOS/racoon-finder",
            "/Applications/.app/Contents/MacOS/racoon-finder",
            // Too short to have a bundle.
            "racoon-finder",
            "/racoon-finder",
            "/MacOS/racoon-finder",
        ] {
            assert_eq!(app_bundle_of(Path::new(odd)), None, "{odd}");
        }
    }

    #[test]
    fn a_translocated_copy_is_blocked_by_its_path() {
        let moved =
            Path::new("/private/var/folders/x/AppTranslocation/ABC-123/d/Racoon Finder.app");
        assert_eq!(trash_block_by_path(moved), Some(TrashBlock::Translocated));
        for fine in [
            "/Applications/Racoon Finder.app",
            "/Users/a/AppTranslocationNot/Racoon.app",
        ] {
            assert_eq!(trash_block_by_path(Path::new(fine)), None, "{fine}");
        }
    }

    #[test]
    fn a_bundle_in_a_folder_that_can_be_written_may_be_trashed() {
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("Racoon.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("racoon-finder");
        std::fs::write(&exe, "x").unwrap();
        let bundle = can_trash_app_bundle(&exe).expect("movable");
        assert_eq!(
            bundle,
            std::fs::canonicalize(dir.path().join("Racoon.app")).unwrap()
        );
        // A program outside a bundle is no candidate, and one that is gone is not either.
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "x").unwrap();
        assert_eq!(can_trash_app_bundle(&plain), Err(TrashBlock::NotInBundle));
    }

    #[cfg(unix)]
    #[test]
    fn a_bundle_in_a_folder_nobody_writes_is_blocked() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("apps");
        let macos = apps.join("Racoon.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("racoon-finder");
        std::fs::write(&exe, "x").unwrap();
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o555)).unwrap();
        // The administrator may write anywhere: the check has nothing to say then.
        let probe = apps.join("probe");
        let as_root = std::fs::write(&probe, "x").is_ok();
        let _ = std::fs::remove_file(&probe);
        let result = can_trash_app_bundle(&exe);
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !as_root {
            assert_eq!(result, Err(TrashBlock::FolderNotWritable));
        }
    }

    #[test]
    fn a_program_run_from_a_build_folder_moves_nothing() {
        // A test runs from a plain program: not an app bundle, or not macOS at all.
        let error = move_app_bundle_to_trash().expect_err("nothing to move");
        let expected = if cfg!(target_os = "macos") {
            io::ErrorKind::InvalidInput
        } else {
            io::ErrorKind::Unsupported
        };
        assert_eq!(error.kind(), expected);
    }
}
