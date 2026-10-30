#![allow(
    unsafe_code,
    reason = "the shell's dialogs and ShellExecuteExW, with COM on their thread"
)]

//! The Windows shell: its dialog of 연결 프로그램 (`SHOpenWithDialog`), its `runas` verb and
//! the start of a terminal (`ShellExecuteExW`), and its properties window
//! (`SHObjectProperties`).
//!
//! Each call runs on a thread of its own that initializes COM as a single-threaded apartment,
//! as the shell wants, and ends it: the caller's thread keeps whatever apartment it has. The
//! shell gets the app's window as the owner ([`Owner`]), so that the dialogs and the consent
//! prompt come in front of it: 연결 프로그램's dialog is modal to it (the window takes no input
//! until the dialog closes), and while a dialog of this thread is owned by the window of the UI
//! thread, the system ties the two threads' input together. Neither waits for the other: the
//! dialog's thread runs the dialog's own message loop, and the UI thread never waits for a
//! thread of this module.
//!
//! `ShellExecuteExW` gets `SEE_MASK_FLAG_NO_UI` (a failure comes back as an error rather than
//! as the shell's message box), `SEE_MASK_NOASYNC` (the call is over when it returns, as the
//! thread ends right after) and, to start a terminal, `SEE_MASK_NO_CONSOLE`: PowerShell gets a
//! console of its own, not the app's (a debug build has one), and no standard handles of the
//! app. A program is given by its absolute path, found in the folders of `PATH`
//! ([`find_program`]): the shell would look in the app's current folder first.

use std::ffi::{OsStr, c_void};
use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    OAIF_EXEC, OPENASINFO, SEE_MASK_FLAG_NO_UI, SEE_MASK_NO_CONSOLE, SEE_MASK_NOASYNC,
    SHELLEXECUTEINFOW, SHOP_FILEPATH, SHObjectProperties, SHOpenWithDialog, ShellExecuteExW,
};
use windows::core::{HRESULT, PCWSTR};

use super::{App, Launch, OpenWith, Owner, find_program, parameters};
use crate::win_util::{to_io, wide};

pub const OPEN_WITH: OpenWith = OpenWith::Dialog;
pub const RUNS_AS_ADMIN: bool = true;

/// `SW_SHOWNORMAL`, from `Win32::UI::WindowsAndMessaging`, whose feature the build leaves out.
const SW_SHOWNORMAL: i32 = 1;

/// Ends the COM use of the thread that began it.
struct ComScope;

impl Drop for ComScope {
    fn drop(&mut self) {
        // SAFETY: only built after `CoInitializeEx` succeeded on this thread, so this balances
        // that one call.
        unsafe { CoUninitialize() };
    }
}

/// Runs `work` on a new thread with COM in a single-threaded apartment, and waits for it.
fn on_shell_thread<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    std::thread::Builder::new()
        .name("rf-platform-shell".into())
        .spawn(move || {
            // SAFETY: no reserved pointer; the thread is new, so no apartment is set yet.
            let init =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            if init.is_err() {
                return Err(to_io(&windows::core::Error::from_hresult(init)));
            }
            // `work` has dropped every COM object it made when it returns, before the scope
            // ends.
            let _scope = ComScope;
            work()
        })?
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("the shell thread panicked")))
}

/// The window handle of `owner`, on the thread that passes it to the shell; `None` for none.
fn owner_window(owner: Owner) -> Option<HWND> {
    owner.raw().map(|hwnd| {
        // A handle is a number that names a window, not memory to read.
        HWND(std::ptr::without_provenance_mut::<c_void>(
            hwnd.get().cast_unsigned(),
        ))
    })
}

/// Whether the user cancelled what the shell asked (the dialog of 연결 프로그램, the consent
/// prompt of `runas`): no failure.
fn cancelled(error: &windows::core::Error) -> bool {
    error.code() == HRESULT::from_win32(ERROR_CANCELLED.0)
}

/// What `ShellExecuteExW` is to do: its verb (`None`: the default one), the file, its
/// parameters and the folder it starts in, as NUL-terminated UTF-16, and its mask.
struct Execute {
    verb: Option<Vec<u16>>,
    file: Vec<u16>,
    parameters: Option<Vec<u16>>,
    directory: Option<Vec<u16>>,
    mask: u32,
    owner: Owner,
}

fn shell_execute(execute: Execute) -> io::Result<()> {
    on_shell_thread(move || {
        let text = |value: &Option<Vec<u16>>| {
            value
                .as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr()))
        };
        let mut info = SHELLEXECUTEINFOW {
            cbSize: u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(u32::MAX),
            fMask: execute.mask,
            lpVerb: text(&execute.verb),
            lpFile: PCWSTR(execute.file.as_ptr()),
            lpParameters: text(&execute.parameters),
            lpDirectory: text(&execute.directory),
            nShow: SW_SHOWNORMAL,
            hwnd: owner_window(execute.owner).unwrap_or_default(),
            ..Default::default()
        };
        // SAFETY: `info` is a whole `SHELLEXECUTEINFOW` with its size set; every string in it
        // is NUL-terminated and owned by `execute`, which outlives the call; the window, if
        // any, is only a handle the call reads (it may have been destroyed at the app's end, and
        // user32 checks a handle before using it: a stale one is no owner); no item ID list is
        // passed, and the mask asks for no process handle back.
        match unsafe { ShellExecuteExW(&raw mut info) } {
            Ok(()) => Ok(()),
            Err(error) if cancelled(&error) => Ok(()),
            Err(error) => Err(to_io(&error)),
        }
    })
}

/// The folders of `PATH`.
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// Whether there is something at `path`. Not `Path::exists`: Windows Terminal's `wt.exe` is an
/// app execution alias, a link that `metadata` cannot follow.
fn present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

pub fn start_terminal(launches: &[Launch], owner: Owner) -> io::Result<()> {
    let dirs = path_dirs();
    // The failure of the last program found, if one would not start: the next one is tried.
    let mut failed = None;
    for launch in launches {
        let Some(program) = find_program(&launch.program, dirs.iter().cloned(), present) else {
            continue;
        };
        let parameters = parameters(&launch.args).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a terminal's arguments must need no quoting",
            )
        })?;
        let execute = Execute {
            verb: None,
            file: wide(program.as_os_str()),
            parameters: (!parameters.is_empty()).then(|| wide(&parameters)),
            directory: launch.cwd.as_deref().map(|cwd| wide(cwd.as_os_str())),
            mask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI | SEE_MASK_NO_CONSOLE,
            owner,
        };
        match shell_execute(execute) {
            Ok(()) => return Ok(()),
            Err(error) => failed = Some(error),
        }
    }
    Err(failed.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "neither Windows Terminal nor PowerShell was found",
        )
    }))
}

pub fn run_as_admin(path: &Path, owner: Owner) -> io::Result<()> {
    // The program starts in its own folder, as File Explorer starts it.
    let directory = path.parent().map(|folder| wide(folder.as_os_str()));
    shell_execute(Execute {
        verb: Some(wide(OsStr::new("runas"))),
        file: wide(path.as_os_str()),
        parameters: None,
        directory,
        mask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        owner,
    })
}

pub fn open_with_dialog(path: &Path, owner: Owner) -> io::Result<()> {
    let file = wide(path.as_os_str());
    on_shell_thread(move || {
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            // Open the file with the app chosen; "always use this app" stays off (module).
            oaifInFlags: OAIF_EXEC,
        };
        // SAFETY: `info` and the NUL-terminated `file` it points to outlive the call, which
        // returns once the dialog is closed; the owner, if any, is only a handle the call reads,
        // which user32 checks before using (the app's window may be gone at its end).
        match unsafe { SHOpenWithDialog(owner_window(owner), &raw const info) } {
            Ok(()) => Ok(()),
            Err(error) if cancelled(&error) => Ok(()),
            Err(error) => Err(to_io(&error)),
        }
    })
}

pub fn show_properties(path: &Path, owner: Owner) -> io::Result<()> {
    let name = wide(path.as_os_str());
    on_shell_thread(move || {
        // SAFETY: `name` is NUL-terminated and outlives the call; the owner, if any, is only a
        // handle the call reads, which user32 checks before using (the app's window may be gone
        // at its end); no page name is passed. The shell shows the window on a thread of its
        // own and returns.
        let shown = unsafe {
            SHObjectProperties(
                owner_window(owner),
                SHOP_FILEPATH,
                PCWSTR(name.as_ptr()),
                PCWSTR::null(),
            )
        };
        if shown.as_bool() {
            Ok(())
        } else {
            Err(io::Error::other(
                "the shell did not show the properties window",
            ))
        }
    })
}

pub fn apps_for(_path: &Path) -> io::Result<Vec<App>> {
    Err(unsupported())
}

pub fn open_with_app(_path: &Path, _app: &Path) -> io::Result<()> {
    Err(unsupported())
}

/// Windows chooses the app in its own dialog ([`open_with_dialog`]).
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows chooses the app in its own dialog",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_choice_is_no_failure() {
        let error = windows::core::Error::from_hresult(HRESULT::from_win32(ERROR_CANCELLED.0));
        assert!(cancelled(&error));
        let error = windows::core::Error::from_hresult(HRESULT::from_win32(2));
        assert!(!cancelled(&error));
    }

    #[test]
    fn the_owner_is_the_window_handle_as_it_was_given() {
        assert_eq!(owner_window(Owner::NONE), None);
        let hwnd = std::num::NonZeroIsize::new(0x0012_3456).expect("not zero");
        let window = owner_window(Owner::hwnd(hwnd)).expect("a window");
        assert_eq!(window.0.addr(), 0x0012_3456);
    }

    #[test]
    fn a_missing_program_starts_nothing() {
        let launch = Launch {
            program: OsStr::new("rf-no-such-terminal.exe").to_owned(),
            args: Vec::new(),
            cwd: Some(std::env::temp_dir()),
        };
        let error = start_terminal(&[launch], Owner::NONE).expect_err("no such program");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }
}
