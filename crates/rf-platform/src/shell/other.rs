//! Systems other than Windows and macOS (Linux, for development): a terminal starts in the
//! folder, as `$TERMINAL` or `x-terminal-emulator`; nothing else is supported.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use super::{App, Launch, OpenWith, find_program};

pub const OPEN_WITH: OpenWith = OpenWith::Unsupported;
pub const RUNS_AS_ADMIN: bool = false;

fn unsupported(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} is not supported on this system"),
    )
}

/// Starts the first program of `launches` that is on `PATH` (or at its absolute path), in its
/// folder, without waiting for it: a terminal lives as long as its window. A thread waits for
/// it, so that it leaves no zombie behind. `Unsupported` when none is there.
pub fn start_terminal(launches: &[Launch]) -> io::Result<()> {
    let dirs: Vec<_> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    start_first(launches, &dirs)
}

/// [`start_terminal`] with the folders of `PATH` given.
fn start_first(launches: &[Launch], dirs: &[std::path::PathBuf]) -> io::Result<()> {
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
        let mut child = command.spawn()?;
        std::thread::Builder::new()
            .name("rf-platform-terminal".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no terminal program was found",
    ))
}

pub fn show_properties(_path: &Path) -> io::Result<()> {
    Err(unsupported("the system's properties window"))
}

pub fn open_with_dialog(_path: &Path) -> io::Result<()> {
    Err(unsupported("choosing the app that opens a file"))
}

pub fn apps_for(_path: &Path) -> io::Result<Vec<App>> {
    Err(unsupported("listing the apps that open a file"))
}

pub fn open_with_app(_path: &Path, _app: &Path) -> io::Result<()> {
    Err(unsupported("opening a file with another app"))
}

pub fn run_as_admin(_path: &Path) -> io::Result<()> {
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
}
