#![allow(
    unsafe_code,
    reason = "the shell's IShellLinkW and IPersistFile, the reparse point ioctls of a junction, and the registry and token reads of symlink_needs_privilege"
)]

//! Links on Windows: a `.lnk` file through the shell's `IShellLinkW` saved with `IPersistFile`
//! (on a thread of its own, with COM in an apartment of its own),
//! a junction (`CreateDirectory`, then `FSCTL_SET_REPARSE_POINT` with the buffer of
//! [`mount_point_buffer`]) and the rights a symbolic link needs.

use std::ffi::{OsString, c_void};
use std::fs::{self, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_GET_REPARSE_POINT, FSCTL_SET_REPARSE_POINT};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{IUnknown, Interface, PCWSTR, w};

use super::{mount_point_buffer, mount_point_target};
use crate::shell::enter_apartment;
use crate::win_util::{to_io, wide};

/// `FILE_FLAG_BACKUP_SEMANTICS`, which opening a folder needs, and `FILE_FLAG_OPEN_REPARSE_POINT`,
/// which opens the link itself rather than what it points to.
const OPEN_THE_FOLDER_ITSELF: u32 = 0x0200_0000 | 0x0020_0000;

/// `FILE_READ_ATTRIBUTES`, all that reading a reparse point takes.
const FILE_READ_ATTRIBUTES: u32 = 0x80;

/// `MAXIMUM_REPARSE_DATA_BUFFER_SIZE`.
const REPARSE_BUFFER: usize = 16 * 1024;

/// `SLGP_RAWPATH`: the path as the shortcut keeps it, unexpanded.
const SLGP_RAWPATH: u32 = 4;

/// `MAX_PATH` of a path with the extended prefix, in UTF-16 units.
const LONG_PATH_UNITS: usize = 32_768;

/// Runs `work` on a thread of its own with COM in a single-threaded apartment, and waits for it:
/// the job's worker thread is not ours to give an apartment (one it entered another way would
/// refuse this one), as the shell's and the trash's own calls do not use the caller's either.
/// Every COM object `work` makes is released before the apartment ends.
fn on_com_thread<T: Send>(work: impl FnOnce() -> io::Result<T> + Send) -> io::Result<T> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("rf-platform-links".into())
            .spawn_scoped(scope, || {
                let _scope = enter_apartment()?;
                work()
            })?
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the links thread panicked")))
    })
}

pub(super) fn make_shortcut(target: &Path, at: &Path) -> io::Result<()> {
    on_com_thread(|| {
        let (target_units, at_units) = (wide(target.as_os_str()), wide(at.as_os_str()));
        // SAFETY: a class of the shell, created in this thread's apartment, which ends after
        // every object below is released.
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
                .map_err(|e| to_io(&e))?;
        // SAFETY: `target_units` and `at_units` are NUL-terminated and outlive these calls, which
        // copy what they are given.
        unsafe {
            link.SetPath(PCWSTR(target_units.as_ptr()))
                .map_err(|e| to_io(&e))?;
            // A shortcut to a folder starts in it, as File Explorer's does.
            if target.is_dir() {
                link.SetWorkingDirectory(PCWSTR(target_units.as_ptr()))
                    .map_err(|e| to_io(&e))?;
            }
        }
        let file: IPersistFile = link.cast().map_err(|e| to_io(&e))?;
        // SAFETY: as above. `Save` replaces a file that is there; the caller made sure none is.
        unsafe { file.Save(PCWSTR(at_units.as_ptr()), true) }.map_err(|e| to_io(&e))
    })
}

pub(super) fn read_shortcut(at: &Path) -> io::Result<Option<PathBuf>> {
    let is_lnk = at
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"));
    if !is_lnk {
        return Ok(None);
    }
    on_com_thread(|| {
        let at_units = wide(at.as_os_str());
        // SAFETY: as in `make_shortcut`.
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
                .map_err(|e| to_io(&e))?;
        let file: IPersistFile = link.cast().map_err(|e| to_io(&e))?;
        // SAFETY: `at_units` is NUL-terminated and outlives the call. A file that is no
        // shortcut fails to load, which is no link.
        if unsafe { file.Load(PCWSTR(at_units.as_ptr()), STGM_READ) }.is_err() {
            return Ok(None);
        }
        let mut path = vec![0u16; LONG_PATH_UNITS];
        // SAFETY: `path` is a buffer of the length it is told; no find data is asked for.
        let got = unsafe { link.GetPath(&mut path, std::ptr::null_mut(), SLGP_RAWPATH) };
        if got.is_err() {
            return Ok(None);
        }
        let end = path
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(path.len());
        // A shortcut that has no path of its own (it points to an item that has no file path)
        // is not a link to a path.
        Ok((end > 0).then(|| PathBuf::from(OsString::from_wide(&path[..end]))))
    })
}

pub(super) fn make_junction(target: &Path, at: &Path) -> io::Result<()> {
    if !target.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a junction points to a folder that exists",
        ));
    }
    // The target as Windows spells it (absolute, with no `.` or `..`), and the buffer first,
    // so that nothing is made for a target no junction can name.
    let target = fs::canonicalize(target)?;
    let buffer = mount_point_buffer(&target)?;
    fs::create_dir(at)?;
    let set = set_reparse_point(at, &buffer);
    if let Err(error) = set {
        // The folder is ours and empty; take it away again.
        let _ = fs::remove_dir(at);
        return Err(error);
    }
    Ok(())
}

fn set_reparse_point(at: &Path, buffer: &[u8]) -> io::Result<()> {
    let folder = OpenOptions::new()
        .write(true)
        .custom_flags(OPEN_THE_FOLDER_ITSELF)
        .open(at)?;
    let size = u32::try_from(buffer.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the reparse data is too long"))?;
    // SAFETY: the handle is the open folder's, valid until `folder` drops after the call;
    // `buffer` is the length it is told, and no output buffer is asked for.
    unsafe {
        DeviceIoControl(
            HANDLE(folder.as_raw_handle()),
            FSCTL_SET_REPARSE_POINT,
            Some(buffer.as_ptr().cast::<c_void>()),
            size,
            None,
            0,
            None,
            None,
        )
    }
    .map_err(|e| to_io(&e))
}

/// The target of the junction at `at`; `None` for a reparse point that is no junction (a
/// symbolic link, a cloud placeholder).
pub(super) fn junction_target(at: &Path) -> io::Result<Option<PathBuf>> {
    let link = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(OPEN_THE_FOLDER_ITSELF)
        .open(at)?;
    let mut buffer = vec![0u8; REPARSE_BUFFER];
    let mut returned = 0u32;
    // SAFETY: the handle is the open link's, valid until `link` drops; `buffer` is the length
    // it is told, and `returned` is a place for a count.
    unsafe {
        DeviceIoControl(
            HANDLE(link.as_raw_handle()),
            FSCTL_GET_REPARSE_POINT,
            None,
            0,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            u32::try_from(buffer.len()).unwrap_or(u32::MAX),
            Some(&raw mut returned),
            None,
        )
    }
    .map_err(|e| to_io(&e))?;
    let filled = buffer.get(..returned as usize).unwrap_or(&buffer);
    Ok(mount_point_target(filled).map(|units| PathBuf::from(OsString::from_wide(&units))))
}

/// Whether a symbolic link cannot be made now: Developer Mode is off and the process is not
/// elevated.
pub(super) fn symlink_needs_privilege() -> bool {
    !(developer_mode() || elevated())
}

/// Whether Developer Mode is on, which lets a symbolic link be made without elevation
/// (`AllowDevelopmentWithoutDevLicense` of `AppModelUnlock`).
fn developer_mode() -> bool {
    let mut value = 0u32;
    let mut size = 4u32;
    // SAFETY: a value of the machine's registry, read into a `u32` of the size it is told.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock"),
            w!("AllowDevelopmentWithoutDevLicense"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast::<c_void>()),
            Some(&raw mut size),
        )
    };
    status == ERROR_SUCCESS && value == 1
}

/// Whether the process runs elevated (its token's `TokenElevation`).
fn elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of this process, and a place for the token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: the token was just opened for query; the output is a `TOKEN_ELEVATION` of the
    // size it is told.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast::<c_void>()),
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(0),
            &raw mut returned,
        )
    };
    // SAFETY: the handle was opened above and nothing uses it after this.
    let _ = unsafe { CloseHandle(token) };
    read.is_ok() && elevation.TokenIsElevated != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rights_of_a_symbolic_link_can_be_told() {
        // Either answer is right; the reads must not fail or panic.
        let _ = symlink_needs_privilege();
    }

    #[test]
    fn only_lnk_files_are_read_as_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "x").unwrap();
        assert_eq!(read_shortcut(&file).unwrap(), None);
    }
}
