//! The system's own windows and programs for real: a terminal, the properties window, the
//! choice of an app, a program run as administrator. They open windows that someone has to look
//! at and close, so they are ignored everywhere and run by hand on a desktop (`cargo test -p
//! rf-platform --test shell -- --ignored --test-threads=1`); CI does not run them. What each one
//! should show is in `docs/ux/keyboard.md` ("연결 프로그램, 터미널, OS 속성 창").
//!
//! Linux, where only the terminal is supported, says so for the rest, which runs everywhere.

use std::path::{Path, PathBuf};

use rf_platform::shell;

/// A folder (and a file in it) under a name a command line would read otherwise: a terminal
/// must open in it, and a window must show it, whole.
fn tricky() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let folder = dir.path().join("a;b & $HOME `x` 100% -d 보고서");
    std::fs::create_dir(&folder).expect("the folder");
    let file = folder.join("메모 (최종);1.txt");
    std::fs::write(&file, "x").expect("the file");
    (dir, folder, file)
}

#[test]
#[ignore = "needs an interactive session"]
fn a_terminal_opens_in_the_folder() {
    let (_dir, folder, _) = tricky();
    shell::open_terminal(&folder).expect("a terminal");
    // By hand: the terminal's prompt is in the folder (`pwd`, `Get-Location`).
}

#[test]
#[ignore = "needs an interactive session"]
fn the_properties_window_shows_the_file() {
    let (_dir, _, file) = tricky();
    shell::show_properties(&file).expect("the window");
    // By hand: File Explorer's properties or Finder's 정보 가져오기 of `메모 (최종);1.txt`.
    // Windows shows it on a thread of the shell's, which ends with the process: look before
    // the test ends.
    std::thread::sleep(std::time::Duration::from_secs(10));
}

#[test]
#[cfg(windows)]
#[ignore = "needs an interactive session"]
fn the_dialog_chooses_the_app() {
    let (_dir, _, file) = tricky();
    // Blocks until the dialog closes: choose an app (the file opens in it), or cancel.
    shell::open_with_dialog(&file).expect("the dialog");
}

#[test]
#[cfg(windows)]
#[ignore = "needs an interactive session"]
fn a_program_runs_as_administrator() {
    let system = std::env::var_os("SystemRoot").expect("SystemRoot");
    let notepad = Path::new(&system).join("System32").join("notepad.exe");
    // By hand: the consent prompt shows; allowed, Notepad runs elevated; refused, nothing fails.
    shell::run_as_admin(&notepad).expect("run as administrator");
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "needs an interactive session"]
fn the_file_opens_with_the_app_chosen() {
    let (_dir, _, file) = tricky();
    let apps = shell::apps_for(&file).expect("the apps");
    let text_edit = apps
        .iter()
        .find(|app| app.path.ends_with("TextEdit.app"))
        .expect("TextEdit opens a text file");
    shell::open_with_app(&file, &text_edit.path).expect("opened");
}

#[test]
#[cfg(not(any(windows, target_os = "macos")))]
fn only_the_terminal_is_supported_here() {
    let (_dir, _, file) = tricky();
    let unsupported = |result: std::io::Result<()>| {
        assert_eq!(
            result.expect_err("unsupported").kind(),
            std::io::ErrorKind::Unsupported
        );
    };
    unsupported(shell::show_properties(&file));
    unsupported(shell::open_with_dialog(&file));
    unsupported(shell::apps_for(&file).map(|_| ()));
    unsupported(shell::open_with_app(&file, Path::new("/usr/bin/true")));
    unsupported(shell::run_as_admin(&file.with_extension("exe")));
    // Not a folder: no terminal is looked for.
    assert_eq!(
        shell::open_terminal(&file).expect_err("a file").kind(),
        std::io::ErrorKind::NotADirectory
    );
}

#[test]
fn a_relative_path_is_refused_everywhere() {
    let relative = Path::new("relative.txt");
    for result in [
        shell::show_properties(relative),
        shell::open_with_dialog(relative),
        shell::run_as_admin(Path::new("setup.exe")),
    ] {
        assert_eq!(
            result.expect_err("refused").kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
