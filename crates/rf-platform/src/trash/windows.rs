#![allow(
    unsafe_code,
    reason = "IFileOperation COM, the user's SID and SHEmptyRecycleBinW"
)]

//! The Windows recycle bin through the shell's `IFileOperation`, and the bin as a place: its
//! `$I…` records, read from the user's folder of each fixed drive's bin.
//!
//! `IFileOperation` recycles only when it can and deletes permanently when it cannot (a path
//! over `MAX_PATH`, an item larger than the bin allows). A progress sink looks at every delete
//! before it happens and aborts the one that would be permanent.

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rf_core::{Error, ErrorKind, FileId, Result, VPath, VolumeId};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{
    FOF_ALLOWUNDO, FOF_NO_UI, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FileOperation,
    IFileOperation, IFileOperationProgressSink, IShellItem, SHCreateItemFromParsingName,
    SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW, SIGDN_FILESYSPATH,
};
use windows::core::{IUnknown, PCWSTR, PWSTR};

use self::sink::RecycleGuard;
use super::{
    TrashId, TrashItem, check_listed, directly_in, io_error, not_in_trash, parse_recycle_index,
    read_record_bytes, stamp_of,
};
use crate::fileid::file_identity;
use crate::volume::volume_root;
use crate::win_util::{to_io, wide};

/// `DRIVE_FIXED` from `GetDriveTypeW`; kept here so the feature module that defines it
/// (`Win32_System_WindowsProgramming`) stays out of the build.
const DRIVE_FIXED: u32 = 3;

/// Whether the volume of `local` is a fixed local drive. Network, removable and optical drives
/// and UNC paths have no recycle bin that the shell fills for us.
pub fn supported(local: &Path) -> bool {
    let Ok(root) = volume_root(local) else {
        return false;
    };
    // SAFETY: `root` is NUL-terminated and outlives the call.
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED }
}

/// Why a recycle attempt did not finish.
enum Failure {
    /// The progress sink stopped a delete that would have been permanent.
    Refused,
    /// The shell parsed the path to a different item (`report.` becomes `report`), so deleting
    /// it would recycle something the caller did not ask for.
    Redirected,
    Io(io::Error),
}

/// Whether nothing is left at `extended`. Any other answer, an error included, counts as
/// "still there".
fn gone(extended: &Path) -> bool {
    std::fs::symlink_metadata(extended).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
}

/// How long a refused recycle waits before its one retry.
const RETRY_REFUSED_AFTER: Duration = Duration::from_millis(300);

/// The volume and file ID of an item, as `file_identity` reads them.
type Identity = (VolumeId, FileId);

/// Recycles the item at `path`. The shell does the work on a thread of its own, because
/// `CoInitializeEx` ties COM to the calling thread, whose apartment is not ours to change.
///
/// The identity of the item is read first, and every attempt recycles only the item with that
/// identity. An item whose file system reports no usable file ID (an ID of 0 or all ones) is
/// refused, because nothing could confirm that the shell's item is this one.
///
/// A refused recycle is tried once more after 300 ms if the same item, by its identity, is
/// still at `path`. On a fresh user profile the shell sometimes refuses to recycle (it offers a
/// permanent delete instead) while several first recycles run at once and the recycle bin is
/// still being set up; in CI one retry recycled every item refused that way. A refusal changes
/// nothing, so the retry is safe. The cost is that a genuine refusal (a path the bin cannot
/// take, an item too large for it, a policy) is reported 300 ms later. A redirected name and
/// an I/O error are not retried.
pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
    let (Some(local), Some(extended)) = (path.as_local(), path.to_extended_length()) else {
        return Err(Error::Unsupported(format!(
            "trash needs a local path: {path}"
        )));
    };
    let wanted = match file_identity(path) {
        Ok(wanted) => wanted,
        Err(e) if e.kind() == ErrorKind::Unsupported => {
            return Err(Error::Unsupported(format!(
                "cannot recycle {path}: its file system reports no file identity, so the item \
                 the shell would recycle cannot be confirmed; delete permanently instead"
            )));
        }
        Err(Error::Io { source, .. }) => return Err(Error::io("trash", path.clone(), source)),
        Err(e) => return Err(e),
    };
    // Each attempt is a whole recycle on a new thread, the identity check included.
    let attempt = || {
        let local = local.to_path_buf();
        std::thread::spawn(move || recycle(wanted, &local))
            .join()
            .unwrap_or_else(|_| Err(Failure::Io(io::Error::other("the recycle thread panicked"))))
    };
    let mut outcome = attempt();
    // Only while the very item asked for is still there: not if it is gone, and not if another
    // item has taken its name in the meantime.
    if matches!(outcome, Err(Failure::Refused))
        && file_identity(path).is_ok_and(|now| now == wanted)
    {
        // The bin of a fresh profile may refuse first recycles that run at the same time.
        std::thread::sleep(RETRY_REFUSED_AFTER);
        outcome = attempt();
    }
    match outcome {
        // The shell must also have removed the item that was asked for, not another one.
        Ok(Some(location)) if gone(&extended) => Ok(Some(location)),
        Ok(Some(_)) => Err(Error::io(
            "trash",
            path.clone(),
            io::Error::other("the recycle bin reported success but the item is still there"),
        )),
        // The shell reported success without naming a bin entry. `Ok(None)` is not an option
        // here: either the item was deleted for good, or nothing happened.
        Ok(None) => {
            let message = if gone(&extended) {
                "deleted permanently instead of recycled"
            } else {
                "the recycle bin took nothing"
            };
            Err(Error::io("trash", path.clone(), io::Error::other(message)))
        }
        Err(Failure::Refused) => Err(Error::Unsupported(format!(
            "the recycle bin cannot take {path}; delete permanently instead"
        ))),
        Err(Failure::Redirected) => Err(Error::Unsupported(format!(
            "cannot recycle {path}: the shell resolves it to a different item; \
             delete permanently instead"
        ))),
        Err(Failure::Io(e)) => Err(Error::io("trash", path.clone(), e)),
    }
}

/// Ends the COM use of the thread that created it.
struct ComScope;

impl Drop for ComScope {
    fn drop(&mut self) {
        // SAFETY: only built after `CoInitializeEx` succeeded on this thread, so this balances
        // that one call.
        unsafe { CoUninitialize() };
    }
}

/// Runs on its own thread: COM in, work, COM out.
fn recycle(wanted: Identity, local: &Path) -> std::result::Result<Option<PathBuf>, Failure> {
    // SAFETY: no reserved pointer; the thread is new, so no apartment is set yet.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if init.is_err() {
        return Err(Failure::Io(to_io(&windows::core::Error::from_hresult(
            init,
        ))));
    }
    // `run` has dropped every COM object when it returns, before the scope ends.
    let _scope = ComScope;
    run(wanted, local)
}

fn run(wanted: Identity, local: &Path) -> std::result::Result<Option<PathBuf>, Failure> {
    let fail = |e: windows::core::Error| Failure::Io(to_io(&e));
    let name = wide(local.as_os_str());

    // SAFETY: `FileOperation` is the class ID of the shell's file operation object, and the
    // interface type is chosen to match it; COM is initialized on this thread.
    let operation: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None::<&IUnknown>, CLSCTX_ALL) }.map_err(fail)?;
    // SAFETY: `operation` is a live interface pointer.
    unsafe {
        operation
            .SetOperationFlags(FOF_NO_UI | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOFX_EARLYFAILURE)
    }
    .map_err(fail)?;
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let item: IShellItem =
        unsafe { SHCreateItemFromParsingName(PCWSTR(name.as_ptr()), None::<&IBindCtx>) }
            .map_err(fail)?;
    // The shell drops a trailing dot or space while parsing, so the item may be a sibling of the
    // one asked for. Check that it is the very item before anything is queued; a name that
    // cannot be read counts as a mismatch.
    if !file_system_path(&item).is_some_and(|resolved| is_item(wanted, &resolved)) {
        return Err(Failure::Redirected);
    }

    let guard = RecycleGuard::new();
    let sink: IFileOperationProgressSink = guard.to_interface();
    // SAFETY: `operation`, `item` and `sink` are live interface pointers.
    let queued = unsafe { operation.DeleteItem(&item, &sink) };
    // SAFETY: as above.
    let performed = queued.and_then(|()| unsafe { operation.PerformOperations() });
    if guard.get().was_refused() {
        return Err(Failure::Refused);
    }
    performed.map_err(fail)?;
    // SAFETY: as above.
    if unsafe { operation.GetAnyOperationsAborted() }
        .map_err(fail)?
        .as_bool()
    {
        return Err(Failure::Io(io::Error::other(
            "the recycle operation was aborted",
        )));
    }
    Ok(guard.get().take_location())
}

/// The file-system path of `item`, if it has one.
fn file_system_path(item: &IShellItem) -> Option<PathBuf> {
    // SAFETY: `item` is a live interface pointer.
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
    // SAFETY: on success `name` is a NUL-terminated string that the shell allocated.
    let path = PathBuf::from(OsString::from_wide(unsafe { name.as_wide() }));
    // SAFETY: the shell allocated `name` with `CoTaskMemAlloc`, and it is freed only here.
    unsafe { CoTaskMemFree(Some(name.0.cast::<c_void>().cast_const())) };
    Some(path)
}

/// Whether `resolved`, the path the shell gives for its parsed item, is the item whose identity
/// is `wanted`, the one asked for.
///
/// This compares file identities (volume and file index, without following a final link), not
/// text: the shell returns long names where the request may hold 8.3 short names, and it can
/// differ in case, but it also turns `report.` into `report`, which is another file. If the
/// identity of `resolved` cannot be read, the answer is no.
fn is_item(wanted: Identity, resolved: &Path) -> bool {
    VPath::local(resolved)
        .ok()
        .and_then(|resolved| file_identity(&resolved).ok())
        .is_some_and(|found| found == wanted)
}

/// The first two UTF-16 units of a record's name: `$I`.
const RECORD_START: [u16; 2] = [0x24, 0x49];

/// The first two UTF-16 units of an item's name: `$R`.
const ITEM_START: [u16; 2] = [0x24, 0x52];

/// The folders of the current user's recycle bins: `X:\$Recycle.Bin\<user SID>` of every fixed
/// drive that has one. None when the user's SID cannot be read. A bin folder that is a link or a
/// junction is no folder by `symlink_metadata`, so it is never one: it would make what it leads
/// to the bin.
fn bins() -> Vec<PathBuf> {
    let Some(sid) = user_sid() else {
        return Vec::new();
    };
    ('A'..='Z')
        .filter_map(|letter| {
            let root = format!("{letter}:\\");
            let wide_root = wide(OsStr::new(&root));
            // SAFETY: `wide_root` is NUL-terminated and outlives the call.
            let fixed = unsafe { GetDriveTypeW(PCWSTR(wide_root.as_ptr())) } == DRIVE_FIXED;
            let bin = PathBuf::from(format!("{root}$Recycle.Bin\\{sid}"));
            (fixed && std::fs::symlink_metadata(&bin).is_ok_and(|meta| meta.is_dir()))
                .then_some(bin)
        })
        .collect()
}

/// Closes a handle when it goes out of scope.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened for this guard alone and is closed once, here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The SID of the user the process runs as, as text (`S-1-5-21-…`), from the process's token.
fn user_sid() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` gives a pseudo-handle that needs no closing, and `token` is a
    // writable handle that lives through the call.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.ok()?;
    let token = OwnedHandle(token);
    let mut needed = 0_u32;
    // SAFETY: a query of the size alone, with no buffer; it fails with
    // `ERROR_INSUFFICIENT_BUFFER` and sets `needed`.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &raw mut needed) };
    let len = usize::try_from(needed).ok().filter(|&len| len > 0)?;
    // `u64` units keep the `TOKEN_USER` at the start aligned.
    let mut buffer = vec![0_u64; len.div_ceil(8)];
    // SAFETY: `buffer` is writable for at least `needed` bytes and lives through the call.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &raw mut needed,
        )
    }
    .ok()?;
    // SAFETY: on success the buffer starts with a `TOKEN_USER`, aligned for it, whose SID points
    // into the same buffer, which outlives the use of both below.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = PWSTR::null();
    // SAFETY: the SID is valid (see above); on success `text` is a NUL-terminated string the
    // system allocated with `LocalAlloc`.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &raw mut text) }.ok()?;
    // SAFETY: `text` is NUL-terminated (see above).
    let sid = unsafe { text.to_string() }.ok();
    // SAFETY: `text` was allocated with `LocalAlloc` and is freed once, here.
    let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    sid
}

/// The `$I…` record next to the `$R…` item at `location`, by its name; `None` for any other
/// name.
pub(super) fn record_of(location: &Path) -> Option<PathBuf> {
    let units: Vec<u16> = location.file_name()?.encode_wide().collect();
    let rest = units
        .strip_prefix(&ITEM_START)
        .filter(|rest| !rest.is_empty())?;
    let mut record = RECORD_START.to_vec();
    record.extend_from_slice(rest);
    Some(location.with_file_name(OsString::from_wide(&record)))
}

/// Every item of the user's recycle bins: each `$I…` record that reads (see
/// `parse_recycle_index`) and whose `$R…` item is there. A bin that cannot be read is passed
/// over, and so is an item without a record, which the shell does not show either.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the shape of `list` on every system; macOS can fail"
)]
pub fn list() -> Result<Vec<TrashItem>> {
    let mut items = Vec::new();
    for bin in bins() {
        let Ok(entries) = std::fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let units: Vec<u16> = entry.file_name().encode_wide().collect();
            let Some(rest) = units.strip_prefix(&RECORD_START) else {
                continue;
            };
            let Some(bytes) = read_record_bytes(&entry.path()) else {
                continue;
            };
            let Some((original, deleted, size)) = parse_recycle_index(&bytes) else {
                continue;
            };
            let mut item_name = ITEM_START.to_vec();
            item_name.extend_from_slice(rest);
            let location = bin.join(OsString::from_wide(&item_name));
            let Ok(meta) = std::fs::symlink_metadata(&location) else {
                continue;
            };
            let Ok(stamp) = stamp_of(&location, Some(&bytes)) else {
                continue;
            };
            let name = original
                .file_name()
                .or_else(|| location.file_name())
                .map(ToOwned::to_owned)
                .unwrap_or_default();
            items.push(TrashItem {
                id: TrashId::of(&location),
                name,
                original: Some(original),
                deleted,
                size: Some(size),
                is_dir: meta.is_dir(),
                location,
                stamp: Some(stamp),
            });
        }
    }
    Ok(items)
}

/// Refuses, with `Error::Unsupported`, an item that is not a `$R…` item directly in one of the
/// user's bins, and then one that is not the item listed: by the item's file ID and the bytes of
/// its `$I…` record (see `check_listed`).
fn check(item: &TrashItem) -> Result<()> {
    let record = record_of(&item.location);
    let Some(record) = record.filter(|_| directly_in(&item.location, &bins())) else {
        return Err(not_in_trash(&item.location));
    };
    check_listed(item, Some(&record))
}

pub fn restore(item: &TrashItem, to: &Path) -> Result<()> {
    check(item)?;
    let location = VPath::local(&item.location)?;
    crate::fsops::rename_no_replace(&location, &VPath::local(to)?)?;
    super::remove_recycle_record(&location);
    Ok(())
}

pub fn delete_forever(item: &TrashItem) -> Result<()> {
    check(item)?;
    let location = VPath::local(&item.location)?;
    let extended = location
        .to_extended_length()
        .unwrap_or_else(|| item.location.clone());
    super::remove_tree(&extended).map_err(|e| io_error("delete_forever", &item.location, e))?;
    super::remove_recycle_record(&location);
    Ok(())
}

/// Empties every recycle bin with `SHEmptyRecycleBinW`, on a thread of its own for COM, with no
/// question, progress or sound. The shell fails this call when the bins are empty already
/// (`E_UNEXPECTED` on some versions); a failure after which the user's bins hold nothing counts
/// as done.
pub fn empty() -> Result<()> {
    let emptied = std::thread::spawn(|| {
        // SAFETY: no reserved pointer; the thread is new, so no apartment is set yet.
        let init =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if init.is_err() {
            return Err(to_io(&windows::core::Error::from_hresult(init)));
        }
        let _scope = ComScope;
        let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
        // SAFETY: no owner window, and a null root path, which means every bin.
        unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), flags) }.map_err(|e| to_io(&e))
    })
    .join()
    .unwrap_or_else(|_| Err(io::Error::other("the thread that empties the bin panicked")));
    match emptied {
        Ok(()) => Ok(()),
        Err(_) if list().is_ok_and(|items| items.is_empty()) => Ok(()),
        Err(e) => {
            let bin = bins()
                .into_iter()
                .next()
                .unwrap_or_else(|| PathBuf::from("C:\\"));
            Err(io_error("empty_trash", &bin, e))
        }
    }
}

/// The progress sink, in a module of its own because the `implement` macro writes code that
/// trips two pedantic lints.
#[allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    reason = "code generated by the `implement` macro"
)]
mod sink {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use windows::Win32::Foundation::E_ABORT;
    use windows::Win32::UI::Shell::{
        IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
        TSF_DELETE_RECYCLE_IF_POSSIBLE,
    };
    use windows::core::{ComObject, HRESULT, PCWSTR, Ref, implement};

    use super::file_system_path;

    /// A progress sink that lets a delete go ahead only if the shell will recycle the item, and
    /// records where the item lands.
    #[implement(IFileOperationProgressSink)]
    pub(super) struct RecycleGuard {
        /// Set when a delete that would be permanent was stopped.
        refused: Cell<bool>,
        /// The file-system path of the item inside the recycle bin.
        location: RefCell<Option<PathBuf>>,
    }

    impl RecycleGuard {
        pub(super) fn new() -> ComObject<Self> {
            ComObject::new(Self {
                refused: Cell::new(false),
                location: RefCell::new(None),
            })
        }

        /// Whether a delete that would have been permanent was stopped.
        pub(super) fn was_refused(&self) -> bool {
            self.refused.get()
        }

        /// Where the recycled item went, once.
        pub(super) fn take_location(&self) -> Option<PathBuf> {
            self.location.take()
        }
    }

    impl IFileOperationProgressSink_Impl for RecycleGuard_Impl {
        fn StartOperations(&self) -> windows::core::Result<()> {
            Ok(())
        }

        fn FinishOperations(&self, _hrresult: HRESULT) -> windows::core::Result<()> {
            Ok(())
        }

        fn PreRenameItem(
            &self,
            _dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn PostRenameItem(
            &self,
            _dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
            _hrrename: HRESULT,
            _psinewlycreated: Ref<'_, IShellItem>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn PreMoveItem(
            &self,
            _dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
            _psidestinationfolder: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn PostMoveItem(
            &self,
            _dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
            _psidestinationfolder: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
            _hrmove: HRESULT,
            _psinewlycreated: Ref<'_, IShellItem>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn PreCopyItem(
            &self,
            _dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
            _psidestinationfolder: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn PostCopyItem(
            &self,
            _dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
            _psidestinationfolder: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
            _hrcopy: HRESULT,
            _psinewlycreated: Ref<'_, IShellItem>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn PreDeleteItem(
            &self,
            dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
        ) -> windows::core::Result<()> {
            // Without the recycle bit the shell is about to delete the item for good.
            let recycles = u32::try_from(TSF_DELETE_RECYCLE_IF_POSSIBLE.0)
                .is_ok_and(|recycle| dwflags & recycle != 0);
            if recycles {
                Ok(())
            } else {
                self.refused.set(true);
                Err(E_ABORT.into())
            }
        }

        fn PostDeleteItem(
            &self,
            _dwflags: u32,
            _psiitem: Ref<'_, IShellItem>,
            hrdelete: HRESULT,
            psinewlycreated: Ref<'_, IShellItem>,
        ) -> windows::core::Result<()> {
            if hrdelete.is_ok()
                && let Some(location) = psinewlycreated.as_ref().and_then(file_system_path)
            {
                *self.location.borrow_mut() = Some(location);
            }
            Ok(())
        }

        fn PreNewItem(
            &self,
            _dwflags: u32,
            _psidestinationfolder: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn PostNewItem(
            &self,
            _dwflags: u32,
            _psidestinationfolder: Ref<'_, IShellItem>,
            _psznewname: &PCWSTR,
            _psztemplatename: &PCWSTR,
            _dwfileattributes: u32,
            _hrnew: HRESULT,
            _psinewitem: Ref<'_, IShellItem>,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn UpdateProgress(&self, _iworktotal: u32, _iworksofar: u32) -> windows::core::Result<()> {
            Ok(())
        }

        fn ResetTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }

        fn PauseTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }

        fn ResumeTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
    }
}
