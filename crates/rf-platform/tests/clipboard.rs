//! The system clipboard for real. On Windows and macOS these tests change the user's
//! clipboard and need a desktop session, which a CI runner may not have, so they are ignored
//! there and run by hand (`cargo test -p rf-platform --test clipboard -- --ignored
//! --test-threads=1`); on Linux the clipboard is this process's own and they always run.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rf_platform::clipboard::{self, ClipboardFiles, DropEffect};

/// One test at a time: they share the clipboard.
static CLIPBOARD: Mutex<()> = Mutex::new(());

fn hold() -> MutexGuard<'static, ()> {
    CLIPBOARD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Paths in the temporary folder, under names a clipboard must keep as they are: Korean
/// names composed (NFC) and one decomposed (NFD), which must each come back in their own form.
fn paths() -> Vec<PathBuf> {
    let dir = std::env::temp_dir();
    vec![
        dir.join("보고서 (최종).txt"),
        dir.join("a b%20#?.txt"),
        dir.join("폴더").join("안쪽"),
        // `보고서` decomposed.
        dir.join("\u{1107}\u{1169}\u{1100}\u{1169}\u{1109}\u{1165}.txt"),
    ]
}

/// The bytes of each path, which the clipboard must give back exactly.
fn bytes(paths: &[PathBuf]) -> Vec<&[u8]> {
    paths
        .iter()
        .map(|path| path.as_os_str().as_encoded_bytes())
        .collect()
}

#[test]
#[cfg_attr(
    any(windows, target_os = "macos"),
    ignore = "needs an interactive session"
)]
fn files_put_on_the_clipboard_read_back_with_their_count() {
    let _held = hold();
    let paths = paths();
    let change = clipboard::put_files(&paths, DropEffect::Move).expect("put");
    assert_eq!(clipboard::change_count(), Some(change));
    // The menus' look, without reading them.
    assert!(clipboard::has_files());
    let read = clipboard::get_files().expect("read").expect("files");
    // Byte for byte: no name changes its Unicode form on the way.
    assert_eq!(bytes(&read.paths), bytes(&paths));
    // macOS keeps no effect: a move there is Finder's ⌥⌘V, or the app's own mark.
    let effect = if cfg!(target_os = "macos") {
        None
    } else {
        Some(DropEffect::Move)
    };
    assert_eq!(
        read,
        ClipboardFiles {
            paths: paths.clone(),
            effect,
            change,
            skipped: 0,
        }
    );
    // A copy in their place: the count moves on.
    let again = clipboard::put_files(&paths[..1], DropEffect::Copy).expect("put");
    assert_ne!(again, change);
    let read = clipboard::get_files().expect("read").expect("files");
    assert_eq!(read.paths, paths[..1]);
    assert_eq!(
        read.effect,
        (!cfg!(target_os = "macos")).then_some(DropEffect::Copy)
    );
}

#[test]
#[cfg_attr(
    any(windows, target_os = "macos"),
    ignore = "needs an interactive session"
)]
fn the_clipboard_is_emptied_only_while_it_holds_what_was_read() {
    let _held = hold();
    let paths = paths();
    let first = clipboard::put_files(&paths, DropEffect::Move).expect("put");
    let second = clipboard::put_files(&paths, DropEffect::Copy).expect("put");
    // Something else came since the first: it stays.
    assert!(!clipboard::clear_if_unchanged(first).expect("clear"));
    assert!(clipboard::get_files().expect("read").is_some());
    // Nothing came since the second: it goes.
    assert!(clipboard::clear_if_unchanged(second).expect("clear"));
    assert_eq!(clipboard::get_files().expect("read"), None);
    assert!(!clipboard::has_files());
    assert_ne!(clipboard::change_count(), Some(second));
}

#[test]
fn nothing_is_put_for_no_paths() {
    let _held = hold();
    let refused = clipboard::put_files(&[], DropEffect::Copy).expect_err("no paths");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
    let refused = clipboard::put_files(&[PathBuf::new()], DropEffect::Copy).expect_err("empty");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
#[cfg_attr(
    any(windows, target_os = "macos"),
    ignore = "needs an interactive session"
)]
fn text_goes_on_the_clipboard_where_the_system_has_one() {
    let _held = hold();
    let text = "C:\\문서\\a b.txt\r\n\"/Users/me/보고서.txt\"";
    let put = clipboard::put_text(text);
    if cfg!(any(windows, target_os = "macos")) {
        let change = put.expect("put");
        assert_eq!(clipboard::change_count(), Some(change));
        assert_eq!(clipboard::get_text().expect("read").as_deref(), Some(text));
        // Text is no files.
        assert_eq!(clipboard::get_files().expect("read"), None);
        assert!(!clipboard::has_files());
    } else {
        // Linux has no text clipboard of its own here.
        let unsupported = std::io::ErrorKind::Unsupported;
        assert_eq!(put.expect_err("no text").kind(), unsupported);
        assert_eq!(
            clipboard::get_text().expect_err("no text").kind(),
            unsupported
        );
    }
}

#[test]
fn a_path_without_a_network_drive_has_no_share() {
    let local = std::env::temp_dir().join("a.txt");
    assert_eq!(clipboard::universal_path(&local), None);
}
