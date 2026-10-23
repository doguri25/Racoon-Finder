#![allow(unsafe_code, reason = "IFileOperation COM")]

//! The Windows recycle bin through the shell's `IFileOperation`.
//!
//! `IFileOperation` recycles only when it can and deletes permanently when it cannot (a path
//! over `MAX_PATH`, an item larger than the bin allows). A progress sink looks at every delete
//! before it happens and aborts the one that would be permanent.

use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rf_core::{Error, Result, VPath};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::UI::Shell::{
    FOF_ALLOWUNDO, FOF_NO_UI, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FileOperation,
    IFileOperation, IFileOperationProgressSink, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH,
};
use windows::core::{IUnknown, PCWSTR};

use self::sink::RecycleGuard;
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

/// Recycles the item at `path`. The shell does the work on a thread of its own, because
/// `CoInitializeEx` ties COM to the calling thread, whose apartment is not ours to change.
///
/// A refused recycle is tried once more after 300 ms if the item is still there. On a fresh
/// user profile the shell sometimes refuses to recycle (it offers a permanent delete instead)
/// while several first recycles run at once and the recycle bin is still being set up; in CI
/// one retry recycled every item refused that way. A refusal changes nothing, so the retry is
/// safe. The cost is that a genuine refusal (a path the bin cannot take, an item too large for
/// it, a policy) is reported 300 ms later. A redirected name and an I/O error are not retried.
pub fn move_to_trash(path: &VPath) -> Result<Option<PathBuf>> {
    let (Some(local), Some(extended)) = (path.as_local(), path.to_extended_length()) else {
        return Err(Error::Unsupported(format!(
            "trash needs a local path: {path}"
        )));
    };
    // Each attempt is a whole recycle on a new thread, the identity check included.
    let attempt = || {
        let (requested, local) = (path.clone(), local.to_path_buf());
        std::thread::spawn(move || recycle(&requested, &local))
            .join()
            .unwrap_or_else(|_| Err(Failure::Io(io::Error::other("the recycle thread panicked"))))
    };
    let mut outcome = attempt();
    if matches!(outcome, Err(Failure::Refused)) && !gone(&extended) {
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
fn recycle(requested: &VPath, local: &Path) -> std::result::Result<Option<PathBuf>, Failure> {
    // SAFETY: no reserved pointer; the thread is new, so no apartment is set yet.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if init.is_err() {
        return Err(Failure::Io(to_io(&windows::core::Error::from_hresult(
            init,
        ))));
    }
    // `run` has dropped every COM object when it returns, before the scope ends.
    let _scope = ComScope;
    run(requested, local)
}

fn run(requested: &VPath, local: &Path) -> std::result::Result<Option<PathBuf>, Failure> {
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
    if !file_system_path(&item).is_some_and(|resolved| same_item(requested, &resolved)) {
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

/// Whether `resolved`, the path the shell gives for its parsed item, is the item `requested`.
///
/// This compares file identities (volume and file index, without following a final link), not
/// text: the shell returns long names where the request may hold 8.3 short names, and it can
/// differ in case, but it also turns `report.` into `report`, which is another file. If either
/// identity cannot be read, the answer is no.
fn same_item(requested: &VPath, resolved: &Path) -> bool {
    let Ok(resolved) = VPath::local(resolved) else {
        return false;
    };
    matches!(
        (file_identity(requested), file_identity(&resolved)),
        (Ok(wanted), Ok(found)) if wanted == found
    )
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
            if hrdelete.is_ok() {
                if let Some(location) = psinewlycreated.as_ref().and_then(file_system_path) {
                    *self.location.borrow_mut() = Some(location);
                }
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
