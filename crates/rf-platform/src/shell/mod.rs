//! The system's own ways to open an item (PRD OPS-04, OPS-06's system window, OPS-11): the app
//! that opens a file chosen by the user, a program run as administrator, a terminal in a folder,
//! and the system's properties window of an item.
//!
//! - **Windows:** 연결 프로그램 is the shell's dialog (`SHOpenWithDialog`, which opens the file
//!   with the app chosen, `OAIF_EXEC`, and leaves "always use this app" off: a change of the
//!   system's file associations goes through rf-ledger, PRD INST-02); a program runs as
//!   administrator by the shell's `runas` verb (`ShellExecuteExW`; the consent prompt is the
//!   system's); the terminal is Windows Terminal (`wt.exe -d .`), or else PowerShell
//!   (`powershell.exe -NoExit`), each started in the folder; the properties window is the
//!   shell's (`SHObjectProperties`). Every call runs on a thread of its own that initializes
//!   COM, as the trash does, so the caller's apartment is left alone.
//! - **macOS:** the apps that can open a file come from `NSWorkspace`
//!   (`URLsForApplicationsToOpenURL:`, the default one first), for the app to show in a menu of
//!   its own, and the one chosen opens it (`openURLs:withApplicationAtURL:…`); the terminal is
//!   Terminal (`open -a Terminal <folder>`); the properties window is Finder's 정보 가져오기, by
//!   a fixed `AppleScript` that gets the path as its argument (`osascript`). No program runs as
//!   administrator there.
//! - **Elsewhere** (Linux, which is for development): the terminal is `$TERMINAL`, or else
//!   `x-terminal-emulator`, started in the folder; the rest is `io::ErrorKind::Unsupported`.
//!
//! No path is ever put into a command line that another parser reads again: a program gets the
//! folder as the folder it starts in (Windows Terminal reads `.` as that folder, and PowerShell
//! starts there), or as an argument of its own (`open`, `osascript`), which is an absolute path,
//! so it never starts with `-`. Windows Terminal reads `;` as the end of one of its commands even
//! between quotes, and PowerShell reads `$`, backticks and `;` in a `-Command`, which is why the
//! folder goes to neither. A program named by its name alone is looked for in the folders of
//! `PATH` that are absolute paths, never in the app's own current folder. The argument lists
//! are made by pure functions ([`terminal_launches`], [`info_window_launch`]), which the tests
//! check on every system with names that a shell would read otherwise.
//!
//! Every function here blocks: a dialog stays until the user closes it, and the system may take
//! its time to start a program or answer. Call them from a worker thread, never from the UI
//! thread; none of them needs the main thread. A choice the user cancels (the dialog of
//! 연결 프로그램, the consent prompt) is no failure: the call returns `Ok`.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use rf_core::Os;

use crate::natural_cmp;

#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(not(any(windows, target_os = "macos")))]
use self::other as imp;
#[cfg(windows)]
use self::windows as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(windows)]
mod windows;

/// How the system lets the user choose the app that opens a file (연결 프로그램).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpenWith {
    /// The system's own dialog, [`open_with_dialog`] (Windows).
    Dialog,
    /// A list of the apps that can open it, [`apps_for`], for the app to show; the one chosen
    /// opens it, [`open_with_app`] (macOS).
    Apps,
    /// Neither (Linux).
    Unsupported,
}

/// How this system lets the user choose the app that opens a file.
pub const OPEN_WITH: OpenWith = imp::OPEN_WITH;

/// Whether this system runs a program as administrator ([`run_as_admin`]): Windows alone.
pub const RUNS_AS_ADMIN: bool = imp::RUNS_AS_ADMIN;

/// An app that can open a file, as [`apps_for`] finds it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct App {
    /// Its name as the system shows it (`미리보기`, `TextEdit`).
    pub name: String,
    /// Where it is: the path of its bundle.
    pub path: PathBuf,
    /// The system opens the file with it by default.
    pub default: bool,
}

/// A program to start: its name or path, its arguments each as one entry, and the folder it
/// starts in (`None`: wherever).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

/// The extensions of the files Windows runs as administrator, as File Explorer offers it on
/// them: programs (`exe`, `com`), batch files (`bat`, `cmd`) and management consoles (`msc`).
/// An installer (`msi`) asks for the rights it needs itself, so File Explorer has no "run as
/// administrator" on it.
pub const RUNNABLE: [&str; 5] = ["exe", "com", "bat", "cmd", "msc"];

/// Whether `path` names a file that Windows runs as administrator: one of [`RUNNABLE`], in any
/// case. Only the name is looked at.
pub fn can_run_as_admin(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            RUNNABLE
                .iter()
                .any(|runnable| runnable.eq_ignore_ascii_case(extension))
        })
}

/// The programs that open a terminal in `folder` on `os`, the first one that the system has
/// first: Windows Terminal and then PowerShell on Windows, Terminal on macOS, and elsewhere
/// `terminal` (the value of `$TERMINAL`, a program's name or absolute path, when it is one) and
/// then `x-terminal-emulator`. The folder is the folder each program starts in, or (macOS) the
/// argument of its own that `open` hands to Terminal; it is in no other argument.
/// `io::ErrorKind::InvalidInput` for a folder that is not an absolute path of `os`, or that
/// holds a NUL.
pub fn terminal_launches(
    os: Os,
    folder: &Path,
    terminal: Option<&OsStr>,
) -> io::Result<Vec<Launch>> {
    check_path(os, folder)?;
    let here = |program: &str, args: &[&str]| Launch {
        program: OsString::from(program),
        args: args.iter().copied().map(OsString::from).collect(),
        cwd: Some(folder.to_path_buf()),
    };
    Ok(match os {
        // Windows Terminal reads `.` as the folder it starts in: the folder itself would be read
        // by its own parser, which splits at `;` even between quotes.
        Os::Windows => vec![
            here("wt.exe", &["-d", "."]),
            here("powershell.exe", &["-NoExit"]),
        ],
        Os::MacOs => vec![Launch {
            program: OsString::from("/usr/bin/open"),
            args: vec![
                OsString::from("-a"),
                OsString::from("Terminal"),
                folder.as_os_str().to_owned(),
            ],
            cwd: None,
        }],
        Os::Other => {
            let mut launches = Vec::new();
            if let Some(terminal) = terminal.filter(|terminal| !terminal.is_empty()) {
                launches.push(Launch {
                    program: terminal.to_owned(),
                    ..here("", &[])
                });
            }
            launches.push(here("x-terminal-emulator", &[]));
            launches
        }
    })
}

/// The lines of the `AppleScript` that opens Finder's 정보 가져오기 window of the item whose path is
/// its one argument. The path is never part of the script: `osascript` hands it to the script's
/// `run` handler as `item 1 of argv`.
pub const INFO_WINDOW_SCRIPT: [&str; 7] = [
    "on run argv",
    "set target to (POSIX file (item 1 of argv)) as alias",
    "tell application \"Finder\"",
    "activate",
    "open information window of target",
    "end tell",
    "end run",
];

/// The program that opens Finder's 정보 가져오기 window of the item at `path` (macOS):
/// `osascript` with [`INFO_WINDOW_SCRIPT`], a line for each `-e`, and the path as the one
/// argument after them. `io::ErrorKind::InvalidInput` for a path that is not absolute, or that
/// holds a NUL.
pub fn info_window_launch(path: &Path) -> io::Result<Launch> {
    check_path(Os::MacOs, path)?;
    let mut args = Vec::with_capacity(INFO_WINDOW_SCRIPT.len() * 2 + 1);
    for line in INFO_WINDOW_SCRIPT {
        args.push(OsString::from("-e"));
        args.push(OsString::from(line));
    }
    args.push(path.as_os_str().to_owned());
    Ok(Launch {
        program: OsString::from("/usr/bin/osascript"),
        args,
        cwd: None,
    })
}

/// What a failure of `osascript` was, from what it wrote: the system refused to let the app
/// control Finder (`-1743`, `-1744`: 개인정보 보호 및 보안 ▸ 자동화), the item is gone (`-43`), or
/// anything else.
pub fn osascript_failure(stderr: &str) -> io::ErrorKind {
    if stderr.contains("(-1743)") || stderr.contains("(-1744)") {
        io::ErrorKind::PermissionDenied
    } else if stderr.contains("(-43)") {
        io::ErrorKind::NotFound
    } else {
        io::ErrorKind::Other
    }
}

/// The apps of `found` (each with its name and path), as a menu shows them: the system's
/// default `default` first, marked, then the others by their names, each app once.
pub fn arrange_apps(found: Vec<(String, PathBuf)>, default: Option<(String, PathBuf)>) -> Vec<App> {
    let mut apps: Vec<App> = Vec::with_capacity(found.len() + 1);
    let default_path = default.as_ref().map(|(_, path)| path.clone());
    if let Some((name, path)) = default {
        apps.push(App {
            name,
            path,
            default: true,
        });
    }
    let mut others: Vec<(String, PathBuf)> = found
        .into_iter()
        .filter(|(_, path)| Some(path) != default_path.as_ref())
        .collect();
    others.sort_by(|(a, _), (b, _)| {
        natural_cmp(OsStr::new(&a.to_lowercase()), OsStr::new(&b.to_lowercase()))
            .then_with(|| a.cmp(b))
    });
    for (name, path) in others {
        if apps.iter().all(|app| app.path != path) {
            apps.push(App {
                name,
                path,
                default: false,
            });
        }
    }
    apps
}

/// The arguments `args` as one line of parameters for a Windows program that reads its own
/// command line (`ShellExecuteExW`'s `lpParameters`): joined with spaces. `None` when an
/// argument is empty or holds a space, a tab, a quote or a NUL: such an argument would need the
/// quoting of the program that reads it, which this module never relies on.
pub fn parameters(args: &[OsString]) -> Option<OsString> {
    let plain = |arg: &OsString| {
        !arg.is_empty()
            && !arg
                .as_encoded_bytes()
                .iter()
                .any(|byte| matches!(byte, b' ' | b'\t' | b'"' | b'\0'))
    };
    if !args.iter().all(plain) {
        return None;
    }
    Some(args.join(OsStr::new(" ")))
}

/// The first program named `name` in the folders `dirs` (the folders of `PATH`), as `exists`
/// says: `name` itself when it is an absolute path. A folder that is not an absolute path is
/// skipped (an empty entry of `PATH` would mean the current folder), and so is a name with a
/// folder in it that is not absolute: a program is never looked for in the current folder.
pub fn find_program(
    name: &OsStr,
    dirs: impl IntoIterator<Item = PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return exists(path).then(|| path.to_path_buf());
    }
    // A bare name: one part, which is a normal name (not `.` or `..`).
    let mut parts = path.components();
    if !matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    ) {
        return None;
    }
    dirs.into_iter()
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| exists(candidate))
}

/// Opens a terminal in `folder` (OPS-11): the first program of [`terminal_launches`] that the
/// system has. `NotADirectory` when `folder` is not a folder (`NotFound` when it is gone), and
/// `Unsupported` when no terminal program is there.
pub fn open_terminal(folder: &Path) -> io::Result<()> {
    if !std::fs::metadata(folder)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "a terminal opens in a folder",
        ));
    }
    let terminal = std::env::var_os("TERMINAL");
    let launches = terminal_launches(Os::CURRENT, folder, terminal.as_deref())?;
    imp::start_terminal(&launches)
}

/// Shows the system's properties window of the item at `path` (OPS-06): File Explorer's on
/// Windows, Finder's 정보 가져오기 on macOS. It returns once the window is asked for; the window
/// stays until the user closes it. `Unsupported` elsewhere.
pub fn show_properties(path: &Path) -> io::Result<()> {
    check_path(Os::CURRENT, path)?;
    imp::show_properties(path)
}

/// Shows the system's dialog to choose the app that opens the file at `path`, which opens it
/// with the one chosen (Windows), and returns once the dialog closes. `Unsupported` where
/// [`OPEN_WITH`] is not [`OpenWith::Dialog`].
pub fn open_with_dialog(path: &Path) -> io::Result<()> {
    check_path(Os::CURRENT, path)?;
    imp::open_with_dialog(path)
}

/// The apps that can open the file at `path`, the default one first ([`arrange_apps`]).
/// `Unsupported` where [`OPEN_WITH`] is not [`OpenWith::Apps`].
pub fn apps_for(path: &Path) -> io::Result<Vec<App>> {
    check_path(Os::CURRENT, path)?;
    imp::apps_for(path)
}

/// Opens the file at `path` with the app at `app` (one of [`apps_for`]), and returns once the
/// system says it did, or after a while if it does not say. `Unsupported` where
/// [`OPEN_WITH`] is not [`OpenWith::Apps`].
pub fn open_with_app(path: &Path, app: &Path) -> io::Result<()> {
    check_path(Os::CURRENT, path)?;
    check_path(Os::CURRENT, app)?;
    imp::open_with_app(path, app)
}

/// Runs the program at `path` as administrator (Windows): the system asks the user for
/// consent first. `InvalidInput` for a file that is not one of [`RUNNABLE`], and
/// `Unsupported` where [`RUNS_AS_ADMIN`] is false.
pub fn run_as_admin(path: &Path) -> io::Result<()> {
    check_path(Os::CURRENT, path)?;
    if !can_run_as_admin(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only a program runs as administrator",
        ));
    }
    imp::run_as_admin(path)
}

/// `InvalidInput` unless `path` is an absolute path of `os` without a NUL.
fn check_path(os: Os, path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let absolute = match os {
        // `C:\…` or `C:/…`, or a UNC path (`\\server\share`).
        Os::Windows => {
            bytes.starts_with(b"\\\\")
                || matches!(bytes, [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic())
        }
        Os::MacOs | Os::Other => bytes.starts_with(b"/"),
    };
    if !absolute || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an absolute path without a NUL is needed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names a command line would read otherwise, each as the last folder of a path.
    const TRICKY: [&str; 12] = [
        "two words",
        "it's \"quoted\"",
        "a;b",
        "rock & roll",
        "100%PATH%",
        "$HOME and ${HOME}",
        "back`tick`",
        "-leading-dash",
        "--help",
        "보고서 폴더",
        "semi; wt new-tab cmd",
        "end\\",
    ];

    fn unix(name: &str) -> PathBuf {
        PathBuf::from(format!("/home/me/{name}"))
    }

    fn windows(name: &str) -> PathBuf {
        PathBuf::from(format!("C:\\Users\\me\\{name}"))
    }

    /// A path far longer than `MAX_PATH`.
    fn long(base: &str, separator: char) -> PathBuf {
        let mut path = String::from(base);
        for i in 0..40 {
            path.push(separator);
            path.push_str("긴 폴더 이름 ");
            path.push_str(&i.to_string());
        }
        PathBuf::from(path)
    }

    fn os(text: &str) -> OsString {
        OsString::from(text)
    }

    fn plain(args: &[&str]) -> Vec<OsString> {
        args.iter().copied().map(os).collect()
    }

    #[test]
    fn windows_starts_windows_terminal_then_powershell_in_the_folder() {
        let mut folders: Vec<PathBuf> = TRICKY.iter().map(|name| windows(name)).collect();
        folders.push(long("C:\\Users", '\\'));
        folders.push(PathBuf::from("\\\\server\\share\\a;b"));
        for folder in folders {
            let launches = terminal_launches(Os::Windows, &folder, None).expect("launches");
            assert_eq!(
                launches,
                [
                    Launch {
                        program: os("wt.exe"),
                        args: plain(&["-d", "."]),
                        cwd: Some(folder.clone()),
                    },
                    Launch {
                        program: os("powershell.exe"),
                        args: plain(&["-NoExit"]),
                        cwd: Some(folder.clone()),
                    },
                ],
                "{folder:?}"
            );
            // The folder is in no argument, so neither program's own reading can split it:
            // and the arguments need no quoting to make one line of parameters.
            for launch in &launches {
                assert!(parameters(&launch.args).is_some(), "{launch:?}");
            }
        }
    }

    #[test]
    fn windows_ignores_terminal_variable() {
        let folder = windows("x");
        let launches =
            terminal_launches(Os::Windows, &folder, Some(OsStr::new("evil.exe"))).expect("ok");
        assert_eq!(launches[0].program, "wt.exe");
        assert_eq!(launches.len(), 2);
    }

    #[test]
    fn macos_hands_the_folder_to_terminal_as_an_argument_of_its_own() {
        let mut folders: Vec<PathBuf> = TRICKY.iter().map(|name| unix(name)).collect();
        folders.push(long("/Users", '/'));
        for folder in folders {
            let launches = terminal_launches(Os::MacOs, &folder, None).expect("launches");
            let mut args = plain(&["-a", "Terminal"]);
            args.push(folder.clone().into_os_string());
            assert_eq!(
                launches,
                [Launch {
                    program: os("/usr/bin/open"),
                    args,
                    cwd: None,
                }],
                "{folder:?}"
            );
            // Absolute: it starts with `/`, never with `-`, so `open` takes no option from it.
            assert!(launches[0].args[2].as_encoded_bytes().starts_with(b"/"));
        }
    }

    #[test]
    fn linux_starts_the_terminal_variable_then_the_system_terminal_in_the_folder() {
        let mut folders: Vec<PathBuf> = TRICKY.iter().map(|name| unix(name)).collect();
        folders.push(long("/home", '/'));
        for folder in folders {
            let at = |program: &str| Launch {
                program: os(program),
                args: Vec::new(),
                cwd: Some(folder.clone()),
            };
            assert_eq!(
                terminal_launches(Os::Other, &folder, None).expect("launches"),
                [at("x-terminal-emulator")],
                "{folder:?}"
            );
            assert_eq!(
                terminal_launches(Os::Other, &folder, Some(OsStr::new("kitty"))).expect("ok"),
                [at("kitty"), at("x-terminal-emulator")]
            );
            // `$TERMINAL` is a program's name, not a command line: it is never split.
            assert_eq!(
                terminal_launches(Os::Other, &folder, Some(OsStr::new("foot -e sh"))).expect("ok"),
                [at("foot -e sh"), at("x-terminal-emulator")]
            );
            // An empty one is none.
            assert_eq!(
                terminal_launches(Os::Other, &folder, Some(OsStr::new(""))).expect("ok"),
                [at("x-terminal-emulator")]
            );
        }
    }

    #[test]
    fn a_folder_that_is_not_absolute_or_holds_a_nul_is_refused() {
        let refused = [
            (Os::Windows, "relative\\folder"),
            (Os::Windows, "\\no-drive"),
            (Os::Windows, "C:no-slash"),
            (Os::Windows, "-d"),
            (Os::Windows, "C:\\a\0b"),
            (Os::MacOs, "relative/folder"),
            (Os::MacOs, "-a"),
            (Os::MacOs, "/a\0b"),
            (Os::Other, "relative"),
            (Os::Other, ""),
            (Os::Other, "/a\0b"),
        ];
        for (os, folder) in refused {
            let error = terminal_launches(os, Path::new(folder), None).expect_err(folder);
            assert_eq!(
                error.kind(),
                io::ErrorKind::InvalidInput,
                "{os:?} {folder:?}"
            );
        }
        for (os, folder) in [
            (Os::Windows, "C:\\"),
            (Os::Windows, "d:/x"),
            (Os::Windows, "\\\\server\\share"),
            (Os::MacOs, "/"),
            (Os::Other, "/tmp"),
        ] {
            assert!(
                terminal_launches(os, Path::new(folder), None).is_ok(),
                "{folder}"
            );
        }
    }

    #[test]
    fn finder_gets_the_path_as_the_scripts_argument_never_in_its_text() {
        let mut paths: Vec<PathBuf> = TRICKY.iter().map(|name| unix(name)).collect();
        paths.push(long("/Users", '/'));
        // AppleScript would read these inside a string of the script.
        paths.push(unix("x\" & (do shell script \"touch /tmp/owned\") & \""));
        paths.push(unix("line\nbreak"));
        for path in paths {
            let launch = info_window_launch(&path).expect("launch");
            assert_eq!(launch.program, "/usr/bin/osascript");
            assert_eq!(launch.cwd, None);
            let mut wanted = Vec::new();
            for line in INFO_WINDOW_SCRIPT {
                wanted.push(os("-e"));
                wanted.push(os(line));
            }
            wanted.push(path.clone().into_os_string());
            assert_eq!(launch.args, wanted, "{path:?}");
        }
        // The script names no path of its own.
        assert!(INFO_WINDOW_SCRIPT.iter().all(|line| !line.contains('/')));
        for refused in ["relative", "-e", "/a\0b", ""] {
            let error = info_window_launch(Path::new(refused)).expect_err(refused);
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn osascript_errors_say_what_went_wrong() {
        let refused = "execution error: Not authorized to send Apple events to Finder. (-1743)";
        assert_eq!(osascript_failure(refused), io::ErrorKind::PermissionDenied);
        assert_eq!(
            osascript_failure("execution error: … (-1744)"),
            io::ErrorKind::PermissionDenied
        );
        let gone = "execution error: File /x wasn’t found. (-43)";
        assert_eq!(osascript_failure(gone), io::ErrorKind::NotFound);
        assert_eq!(osascript_failure("(-10000)"), io::ErrorKind::Other);
        assert_eq!(osascript_failure(""), io::ErrorKind::Other);
    }

    #[test]
    fn only_programs_run_as_administrator() {
        for name in [
            "setup.exe",
            "SETUP.EXE",
            "tool.Com",
            "run.bat",
            "run.CMD",
            "devmgmt.msc",
            "보고서 (최종).exe",
        ] {
            assert!(can_run_as_admin(Path::new(name)), "{name}");
        }
        for name in [
            "setup.msi",
            "readme.txt",
            "exe",
            ".exe",
            "folder",
            "archive.exe.zip",
            "",
        ] {
            assert!(!can_run_as_admin(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn the_default_app_comes_first_and_the_others_by_name() {
        let app = |name: &str| {
            (
                name.to_owned(),
                PathBuf::from(format!("/Applications/{name}.app")),
            )
        };
        let arranged = arrange_apps(
            vec![
                app("TextEdit"),
                app("BBEdit"),
                app("미리보기"),
                app("Xcode"),
                app("atom"),
                // The same app twice.
                app("BBEdit"),
            ],
            Some(app("Xcode")),
        );
        let names: Vec<(&str, bool)> = arranged
            .iter()
            .map(|app| (app.name.as_str(), app.default))
            .collect();
        assert_eq!(
            names,
            [
                ("Xcode", true),
                ("atom", false),
                ("BBEdit", false),
                ("TextEdit", false),
                ("미리보기", false),
            ]
        );
        // A default the list left out still comes first; no default, none is marked.
        let arranged = arrange_apps(vec![app("TextEdit")], Some(app("Pages")));
        assert_eq!(arranged[0].name, "Pages");
        assert!(arranged[0].default);
        assert_eq!(arranged.len(), 2);
        assert!(
            arrange_apps(vec![app("TextEdit")], None)
                .iter()
                .all(|app| !app.default)
        );
    }

    #[test]
    fn parameters_join_plain_arguments_only() {
        assert_eq!(parameters(&plain(&["-d", "."])), Some(os("-d .")));
        assert_eq!(parameters(&plain(&["-NoExit"])), Some(os("-NoExit")));
        assert_eq!(parameters(&[]), Some(os("")));
        for bad in ["two words", "tab\there", "\"quoted\"", "", "nul\0"] {
            assert_eq!(parameters(&plain(&["-d", bad])), None, "{bad:?}");
        }
    }

    #[test]
    fn programs_are_looked_for_in_absolute_folders_only() {
        let dirs = || {
            vec![
                PathBuf::new(),
                PathBuf::from("."),
                PathBuf::from("bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/bin"),
            ]
        };
        let everywhere = |_: &Path| true;
        // The first absolute folder that has it.
        assert_eq!(
            find_program(OsStr::new("x-terminal-emulator"), dirs(), everywhere),
            Some(PathBuf::from("/usr/local/bin/x-terminal-emulator"))
        );
        let only_usr_bin = |path: &Path| path.starts_with("/usr/bin");
        assert_eq!(
            find_program(OsStr::new("kitty"), dirs(), only_usr_bin),
            Some(PathBuf::from("/usr/bin/kitty"))
        );
        assert_eq!(find_program(OsStr::new("kitty"), dirs(), |_| false), None);
        // An absolute path is itself, when it is there.
        assert_eq!(
            find_program(OsStr::new("/opt/term"), dirs(), everywhere),
            Some(PathBuf::from("/opt/term"))
        );
        assert_eq!(
            find_program(OsStr::new("/opt/term"), dirs(), |_| false),
            None
        );
        // A relative path with a folder would be read in the current folder.
        assert_eq!(
            find_program(OsStr::new("bin/term"), dirs(), everywhere),
            None
        );
        assert_eq!(find_program(OsStr::new("./term"), dirs(), everywhere), None);
        assert_eq!(find_program(OsStr::new(""), dirs(), everywhere), None);
    }

    #[test]
    fn a_program_runs_as_administrator_only_where_the_system_does_it() {
        assert_eq!(RUNS_AS_ADMIN, cfg!(windows));
        let expected = if cfg!(windows) {
            OpenWith::Dialog
        } else if cfg!(target_os = "macos") {
            OpenWith::Apps
        } else {
            OpenWith::Unsupported
        };
        assert_eq!(OPEN_WITH, expected);
    }
}
