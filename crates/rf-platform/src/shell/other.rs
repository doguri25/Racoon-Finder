//! Systems other than Windows and macOS (Linux, for development): a terminal starts in the
//! folder, as `$TERMINAL` or `x-terminal-emulator`; nothing else is supported.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use super::{App, Launch, OpenWith, Owner, find_program};

pub const OPEN_WITH: OpenWith = OpenWith::Unsupported;
pub const RUNS_AS_ADMIN: bool = false;

fn unsupported(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} is not supported on this system"),
    )
}

/// Starts the first program of `launches` that is on `PATH` (or at its absolute path) and
/// starts, in its folder, without waiting for it: a terminal lives as long as its window. A
/// thread waits for it, so that it leaves no zombie behind. One that is there but does not
/// start (not executable) gives way to the next. `Unsupported` when none is there, and the last
/// failure when none starts. There is no window to own anything here.
pub fn start_terminal(launches: &[Launch], _owner: Owner) -> io::Result<()> {
    let dirs: Vec<_> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    start_first(launches, &dirs)
}

/// [`start_terminal`] with the folders of `PATH` given.
fn start_first(launches: &[Launch], dirs: &[std::path::PathBuf]) -> io::Result<()> {
    let mut failed = None;
    for launch in launches {
        let Some(program) = find_program(&launch.program, dirs.iter().cloned(), Path::is_file)
        else {
            continue;
        };
        let mut command = Command::new(program);
        command
            .args(&launch.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = &launch.cwd {
            command.current_dir(cwd);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                failed = Some(error);
                continue;
            }
        };
        std::thread::Builder::new()
            .name("rf-platform-terminal".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        return Ok(());
    }
    Err(failed.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::Unsupported, "no terminal program was found")
    }))
}

pub fn show_properties(_path: &Path, _owner: Owner) -> io::Result<()> {
    Err(unsupported("the system's properties window"))
}

pub fn open_with_dialog(_path: &Path, _owner: Owner) -> io::Result<()> {
    Err(unsupported("choosing the app that opens a file"))
}

pub fn apps_for(_path: &Path) -> io::Result<Vec<App>> {
    Err(unsupported("listing the apps that open a file"))
}

pub fn open_with_app(_path: &Path, _app: &Path) -> io::Result<()> {
    Err(unsupported("opening a file with another app"))
}

pub fn run_as_admin(_path: &Path, _owner: Owner) -> io::Result<()> {
    Err(unsupported("running a program as administrator"))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    fn launch(program: &str, cwd: &Path) -> Launch {
        Launch {
            program: OsString::from(program),
            args: Vec::new(),
            cwd: Some(cwd.to_path_buf()),
        }
    }

    #[test]
    fn a_missing_terminal_is_passed_over_for_the_next() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let bin = [PathBuf::from("/usr/bin"), PathBuf::from("/bin")];
        // `true` stands in for a terminal: it starts in the folder and ends.
        let launches = [
            launch("rf-no-such-terminal", dir.path()),
            launch("true", dir.path()),
        ];
        start_first(&launches, &bin).expect("the second one starts");
        let none = [launch("rf-no-such-terminal", dir.path())];
        let error = start_first(&none, &bin).expect_err("no terminal");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn a_terminal_that_does_not_start_gives_way_to_the_next() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        // There, but not executable: `execve` refuses it, even to root.
        std::fs::write(dir.path().join("rf-broken-terminal"), "not a program").expect("a file");
        let dirs = [
            dir.path().to_path_buf(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ];
        let launches = [
            launch("rf-broken-terminal", dir.path()),
            launch("true", dir.path()),
        ];
        start_first(&launches, &dirs).expect("the second one starts");
        // None starts: the failure of the one that was there.
        let broken = [launch("rf-broken-terminal", dir.path())];
        let error = start_first(&broken, &dirs).expect_err("it does not start");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
}
