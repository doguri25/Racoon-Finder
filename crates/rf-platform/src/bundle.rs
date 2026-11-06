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
