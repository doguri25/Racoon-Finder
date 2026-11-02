#![allow(
    unsafe_code,
    reason = "the shell's dialogs, ShellExecuteExW, and its menu of items, with COM"
)]

//! The Windows shell: its dialog of 연결 프로그램 (`SHOpenWithDialog`), its `runas` verb and
//! the start of a terminal (`ShellExecuteExW`), its properties window (`SHObjectProperties`),
//! and its own menu of items ([`system_menu`]).
//!
//! Each dialog's call runs on a thread of its own that initializes COM as a single-threaded
//! apartment, as the shell wants, and ends it: the caller's thread keeps whatever apartment it
//! has. The shell gets the app's window as the owner ([`Owner`]), so that the dialogs and the
//! consent prompt come in front of it: 연결 프로그램's dialog is modal to it (the window takes no
//! input until the dialog closes), and while a dialog of this thread is owned by the window of
//! the UI thread, the system ties the two threads' input together. Neither waits for the other:
//! the dialog's thread runs the dialog's own message loop, and the UI thread never waits for a
//! thread of this module.
//!
//! `ShellExecuteExW` gets `SEE_MASK_FLAG_NO_UI` (a failure comes back as an error rather than
//! as the shell's message box), `SEE_MASK_NOASYNC` (the call is over when it returns, as the
//! thread ends right after) and, to start a terminal, `SEE_MASK_NO_CONSOLE`: PowerShell gets a
//! console of its own, not the app's (a debug build has one), and no standard handles of the
//! app. A program is given by its absolute path, found in the folders of `PATH`
//! ([`find_program`]): the shell would look in the app's current folder first.
//!
//! The menu of items is the one File Explorer shows (its "더 많은 옵션 표시"), on the window's
//! own thread, which winit made a single-threaded apartment (OLE):
//!
//! - **What it is of:** the folder's item id list (`SHParseDisplayName`), its `IShellFolder`
//!   (`SHBindToObject`), the item ids of the names in it (`ParseDisplayName`), and their menu
//!   (`GetUIObjectOf`, `IContextMenu`); with no names, the menu of the folder's own view
//!   (`CreateViewObject`), as its part without items shows it. `QueryContextMenu` fills it with
//!   `CMF_NORMAL | CMF_CANRENAME`, and `CMF_EXTENDEDVERBS` for Shift's commands.
//! - **Who shows it:** a hidden window of this module's own class, owned by the app's window
//!   ([`Helper`]), owns the menu (`TrackPopupMenuEx` with `TPM_RETURNCMD | TPM_RIGHTBUTTON`),
//!   after `SetForegroundWindow` of the app's window, so that a click elsewhere closes it. Its
//!   procedure ([`helper_proc`]) hands `WM_INITMENUPOPUP`, `WM_DRAWITEM`, `WM_MEASUREITEM` and
//!   `WM_MENUCHAR` to the menu (`IContextMenu3::HandleMenuMsg2`, else
//!   `IContextMenu2::HandleMenuMsg`), which fills 보내기, 연결 프로그램 and the extensions'
//!   submenus as they open and draws their icons. winit's window and its procedure are left as
//!   they are. Nothing is borrowed while the menu's code runs: the procedure takes a clone of
//!   the menu's interface ([`HOSTED`]), since that code may pump messages and come back.
//! - **What is chosen:** a command whose verb (`GetCommandString(GCS_VERBW)`) the app does
//!   itself ([`app_verb`]: 이름 바꾸기, 삭제, 잘라내기, 복사, 붙여넣기, with Shift as it is held as
//!   the menu closes) comes back to the app ([`SystemMenuChoice::App`]); any other is carried out
//!   by `InvokeCommand` (`CMINVOKECOMMANDINFOEX`: Unicode, `ptInvoke`, the Shift and Ctrl held,
//!   the app's window, `SW_SHOWNORMAL`, the folder). Its own failure is the handler's to tell, as
//!   File Explorer leaves it to it.
//! - **Shell extensions** run in the app's process, on this thread: one that crashes ends the
//!   app (PRD R3; to run them apart is for a later version).

use std::cell::RefCell;
use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::OnceLock;

use windows::Win32::Foundation::{
    ERROR_CANCELLED, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize, IBindCtx,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE,
    CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBW, IContextMenu,
    IContextMenu2, IContextMenu3, ILFree, IShellFolder, OAIF_EXEC, OPENASINFO, SEE_MASK_FLAG_NO_UI,
    SEE_MASK_NO_CONSOLE, SEE_MASK_NOASYNC, SHBindToObject, SHELLEXECUTEINFOW, SHOP_FILEPATH,
    SHObjectProperties, SHOpenWithDialog, SHParseDisplayName, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, HMENU,
    RegisterClassExW, SW_SHOWNORMAL, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HRESULT, Interface, PCSTR, PCWSTR, PSTR, w};

use super::{
    App, Launch, OpenWith, Owner, SystemMenuChoice, WindowPoint, app_verb, find_program, parameters,
};
use crate::dnd::screen_point;
use crate::win_util::{to_io, wide};

pub const OPEN_WITH: OpenWith = OpenWith::Dialog;
pub const RUNS_AS_ADMIN: bool = true;
pub const SYSTEM_MENU: bool = true;
pub const SHARES: bool = false;

/// Ends the COM use of the thread that began it.
struct ComScope;

impl Drop for ComScope {
    fn drop(&mut self) {
        // SAFETY: only built after `CoInitializeEx` succeeded on this thread, so this balances
        // that one call.
        unsafe { CoUninitialize() };
    }
}

/// COM on the calling thread in a single-threaded apartment, until the scope it gives ends: a
/// new thread's first use of it, or one more use of the apartment the thread is already (the
/// window's, where winit initialized OLE). A thread of another apartment is refused.
fn enter_apartment() -> io::Result<ComScope> {
    // SAFETY: no reserved pointer; on a thread that has no apartment yet this makes it one, and
    // on a single-threaded apartment it only counts one more use of it (`S_FALSE`), which the
    // scope gives back.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if init.is_err() {
        return Err(to_io(&windows::core::Error::from_hresult(init)));
    }
    Ok(ComScope)
}

/// Runs `work` on a new thread with COM in a single-threaded apartment, and waits for it.
fn on_shell_thread<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    std::thread::Builder::new()
        .name("rf-platform-shell".into())
        .spawn(move || {
            // `work` has dropped every COM object it made when it returns, before the scope
            // ends.
            let _scope = enter_apartment()?;
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
            nShow: SW_SHOWNORMAL.0,
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

pub fn share(_owner: Owner, _paths: &[PathBuf], _at: WindowPoint) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the app shares items on macOS alone",
    ))
}

/// The ids the commands of the menu get (`QueryContextMenu`): from 1, since `TrackPopupMenuEx`
/// answers 0 for no command, to the last id a menu's item may have.
const FIRST_COMMAND: u32 = 1;
const LAST_COMMAND: u32 = 0x7FFF;

/// `CMIC_MASK_UNICODE` (`SEE_MASK_UNICODE`), which the bindings leave out: the command's verb,
/// parameters, folder and title are `CMINVOKECOMMANDINFOEX`'s wide strings.
const CMIC_MASK_UNICODE: u32 = 0x0000_4000;

/// The class of the hidden window that owns the menu ([`Helper`]).
const HELPER_CLASS: PCWSTR = w!("rf-platform.ShellMenu");

thread_local! {
    /// The menu the hidden window owns while it shows, for the messages of its submenus and of
    /// the items an extension draws itself ([`helper_proc`]).
    static HOSTED: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

pub fn system_menu(
    owner: Owner,
    folder: &Path,
    names: &[OsString],
    at: WindowPoint,
    extended: bool,
) -> io::Result<SystemMenuChoice> {
    let hwnd = owner_window(owner)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no window handle"))?;
    // Given back last, once every COM object below is gone.
    let _com = enter_apartment()?;
    let menu = context_menu(hwnd, folder, names)?;
    // SAFETY: makes an empty menu, which `popup` destroys with the submenus put in it.
    let popup = PopupMenu(unsafe { CreatePopupMenu() }.map_err(|e| to_io(&e))?);
    let flags = CMF_NORMAL | CMF_CANRENAME | if extended { CMF_EXTENDEDVERBS } else { 0 };
    // SAFETY: the empty menu, filled from its first place with ids from the first to the last.
    unsafe { menu.QueryContextMenu(popup.0, 0, FIRST_COMMAND, LAST_COMMAND, flags) }
        .ok()
        .map_err(|e| to_io(&e))?;
    let screen = screen_of(hwnd, at);
    let chosen = {
        let helper = Helper::new(hwnd)?;
        // Let go before the window.
        let _hosted = Hosted::new(Handler::of(&menu));
        // SAFETY: the app's window, on this thread: it is in front, so that a click elsewhere
        // closes the menu.
        let _ = unsafe { SetForegroundWindow(hwnd) };
        // SAFETY: the filled menu, at a point of the screen, owned by the hidden window of this
        // thread; the call returns once the menu has closed, with the id of the command chosen
        // (0 for none).
        unsafe {
            TrackPopupMenuEx(
                popup.0,
                (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                screen.x,
                screen.y,
                helper.0,
                None,
            )
        }
        .0
    };
    let Some(offset) = u32::try_from(chosen)
        .ok()
        .and_then(|id| id.checked_sub(FIRST_COMMAND))
        .and_then(|offset| usize::try_from(offset).ok())
    else {
        return Ok(SystemMenuChoice::Nothing);
    };
    // As they are as the menu closed: the thread's last message tells.
    let shift = key_down(VK_SHIFT);
    let ctrl = key_down(VK_CONTROL);
    if let Some(app) = verb_of(&menu, offset).and_then(|verb| app_verb(&verb, shift)) {
        return Ok(SystemMenuChoice::App(app));
    }
    invoke(
        &menu,
        &Invoke {
            hwnd,
            offset,
            directory: wide(folder.as_os_str()),
            at: screen,
            shift,
            ctrl,
        },
    );
    Ok(SystemMenuChoice::Done)
}

/// The menu of the items `names` of the folder at `folder`, or of the folder's own view for no
/// names, for the window `hwnd`.
fn context_menu(hwnd: HWND, folder: &Path, names: &[OsString]) -> io::Result<IContextMenu> {
    let path = wide(folder.as_os_str());
    let mut id = std::ptr::null_mut();
    // SAFETY: a NUL-terminated path that lives through the call, without a bind context and
    // without attributes asked; the id list it gives is `folder_id`'s to free.
    unsafe {
        SHParseDisplayName(
            PCWSTR(path.as_ptr()),
            None::<&IBindCtx>,
            &raw mut id,
            0,
            None,
        )
    }
    .map_err(|e| to_io(&e))?;
    let folder_id = Pidl::new(id)?;
    // SAFETY: the folder's absolute id list, which `folder_id` keeps alive through the call,
    // bound from the desktop without a bind context.
    let shell_folder: IShellFolder =
        unsafe { SHBindToObject(None::<&IShellFolder>, folder_id.get(), None::<&IBindCtx>) }
            .map_err(|e| to_io(&e))?;
    if names.is_empty() {
        // SAFETY: the menu of the folder's own view, for the app's window.
        return unsafe { shell_folder.CreateViewObject(hwnd) }.map_err(|e| to_io(&e));
    }
    let items = names
        .iter()
        .map(|name| item_id(&shell_folder, hwnd, name))
        .collect::<io::Result<Vec<Pidl>>>()?;
    let ids: Vec<*const ITEMIDLIST> = items.iter().map(Pidl::get).collect();
    // SAFETY: the folder's own ids of its items, which `items` keeps alive through the call,
    // for the app's window.
    unsafe { shell_folder.GetUIObjectOf(hwnd, &ids, None) }.map_err(|e| to_io(&e))
}

/// The id list of the item `name` of `folder`, relative to it.
fn item_id(folder: &IShellFolder, hwnd: HWND, name: &OsStr) -> io::Result<Pidl> {
    let name = wide(name);
    let mut id = std::ptr::null_mut();
    // SAFETY: a NUL-terminated name in the folder that lives through the call, without a bind
    // context; no attributes asked (a null pointer); the id list it gives is the `Pidl`'s to
    // free.
    unsafe {
        folder.ParseDisplayName(
            hwnd,
            None::<&IBindCtx>,
            PCWSTR(name.as_ptr()),
            None,
            &raw mut id,
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| to_io(&e))?;
    Pidl::new(id)
}

/// An item id list (PIDL) the shell allocated, freed as this goes.
struct Pidl(NonNull<ITEMIDLIST>);

impl Pidl {
    /// The id list `id` the shell gave; `NotFound` for none.
    fn new(id: *mut ITEMIDLIST) -> io::Result<Pidl> {
        NonNull::new(id)
            .map(Pidl)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the shell named no item"))
    }

    /// The id list, for a call that reads it while this lives.
    fn get(&self) -> *const ITEMIDLIST {
        self.0.as_ptr().cast_const()
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: an id list the shell allocated with the task allocator, freed once.
        unsafe { ILFree(Some(self.get())) };
    }
}

/// A menu of the system's, destroyed as this goes, with its submenus.
struct PopupMenu(HMENU);

impl Drop for PopupMenu {
    fn drop(&mut self) {
        // SAFETY: a menu `CreatePopupMenu` made, which no window holds once its menu has closed.
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

/// The hidden window of this module's own class that owns the menu, owned by the app's window,
/// destroyed as this goes: the menu sends it the messages of its submenus ([`helper_proc`]).
struct Helper(HWND);

impl Helper {
    /// A hidden window owned by the app's window `owner`, on this thread.
    fn new(owner: HWND) -> io::Result<Helper> {
        // SAFETY: no name: the module the process started from.
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(|e| to_io(&e))?;
        let instance = HINSTANCE::from(module);
        register_helper(instance)?;
        // SAFETY: a registered class of this module, without a title or a menu; a popup that is
        // never shown, of no size, owned by the app's window, on this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                HELPER_CLASS,
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                Some(owner),
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|e| to_io(&e))?;
        Ok(Helper(hwnd))
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // SAFETY: the window this made, on this thread, destroyed once.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Registers the class of the hidden window ([`HELPER_CLASS`]) once for the process.
fn register_helper(instance: HINSTANCE) -> io::Result<()> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let registered = *REGISTERED.get_or_init(|| {
        let class = WNDCLASSEXW {
            cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
            lpfnWndProc: Some(helper_proc),
            hInstance: instance,
            lpszClassName: HELPER_CLASS,
            ..Default::default()
        };
        // SAFETY: a whole class description with its size set, whose procedure and name live as
        // long as the process.
        unsafe { RegisterClassExW(&raw const class) != 0 }
    });
    if registered {
        Ok(())
    } else {
        Err(io::Error::other(
            "the class of the menu's window could not be registered",
        ))
    }
}

/// What answers the messages of the menu: its `IContextMenu3`, else its `IContextMenu2`.
#[derive(Clone)]
enum Handler {
    Three(IContextMenu3),
    Two(IContextMenu2),
}

impl Handler {
    /// The handler of `menu`; `None` for a menu that takes no messages.
    fn of(menu: &IContextMenu) -> Option<Handler> {
        menu.cast::<IContextMenu3>()
            .map(Handler::Three)
            .or_else(|_| menu.cast::<IContextMenu2>().map(Handler::Two))
            .ok()
    }

    /// Hands the menu's `message` to it: what the window answers, or `None` for a message it
    /// does not take (`IContextMenu2` takes no `WM_MENUCHAR`).
    fn handle(&self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match self {
            Handler::Three(menu) => {
                let mut answer = LRESULT(0);
                // SAFETY: a message of the menu with its parameters as the system gave them,
                // and a live place for the answer.
                unsafe { menu.HandleMenuMsg2(message, wparam, lparam, Some(&raw mut answer)) }
                    .ok()?;
                Some(answer)
            }
            Handler::Two(menu) if message != WM_MENUCHAR => {
                // SAFETY: as above.
                unsafe { menu.HandleMenuMsg(message, wparam, lparam) }.ok()?;
                // What a window answers the message it handled.
                Some(LRESULT(isize::from(message != WM_INITMENUPOPUP)))
            }
            Handler::Two(_) => None,
        }
    }
}

/// The menu hosted by the hidden window ([`HOSTED`]) while this lives.
struct Hosted;

impl Hosted {
    fn new(handler: Option<Handler>) -> Hosted {
        let before = HOSTED.with(|hosted| hosted.replace(handler));
        drop(before);
        Hosted
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        // Let go once the cell is no longer borrowed.
        let hosted = HOSTED
            .try_with(|hosted| {
                hosted
                    .try_borrow_mut()
                    .ok()
                    .and_then(|mut held| held.take())
            })
            .ok()
            .flatten();
        drop(hosted);
    }
}

/// The hidden window's procedure: the messages of the menu's submenus and of the items an
/// extension draws itself go to the menu it hosts; the rest, and the ones the menu does not
/// take, to the system's default handling.
unsafe extern "system" fn helper_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if matches!(
        message,
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
    ) {
        // A clone of the handler (one more reference to it), so that nothing is borrowed while
        // the menu's code runs, which may pump messages and come back here; none as the thread
        // ends.
        let handler = HOSTED
            .try_with(|hosted| hosted.try_borrow().ok().and_then(|held| held.clone()))
            .ok()
            .flatten();
        if let Some(answer) = handler.and_then(|handler| handler.handle(message, wparam, lparam)) {
            return answer;
        }
    }
    // SAFETY: the message as the system gave it, to its default handling.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Whether `key` is down, as the thread's last message found it.
fn key_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: only reads the state of the key; its high bit is set while it is down.
    unsafe { GetKeyState(i32::from(key.0)) < 0 }
}

/// The point `at` of the window `hwnd`'s content on the screen, in physical pixels.
fn screen_of(hwnd: HWND, at: WindowPoint) -> POINT {
    let mut origin = POINT::default();
    // SAFETY: the app's window, and a live point the call writes.
    let _ = unsafe { ClientToScreen(hwnd, &raw mut origin) };
    // SAFETY: only reads the window's DPI; 0 for a window that is gone (scale 1).
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let (x, y) = screen_point(at, (origin.x, origin.y), f64::from(dpi) / 96.0);
    POINT { x, y }
}

/// The verb of the menu's command `offset` (`GCS_VERBW`); `None` for a command without one, or
/// a handler that does not tell.
fn verb_of(menu: &IContextMenu, offset: usize) -> Option<String> {
    let mut verb = [0u16; 256];
    let room = u32::try_from(verb.len()).unwrap_or(0);
    // SAFETY: room for `room` UTF-16 units, into which the call writes the verb, ended by a NUL.
    unsafe {
        menu.GetCommandString(
            offset,
            GCS_VERBW,
            None,
            PSTR(verb.as_mut_ptr().cast()),
            room,
        )
    }
    .ok()?;
    let end = verb.iter().position(|&unit| unit == 0)?;
    String::from_utf16(&verb[..end]).ok()
}

/// What [`invoke`] carries out: the menu's command `offset`, for the app's window `hwnd`, in the
/// folder `directory` (NUL-terminated UTF-16), where the menu showed (`at`, on the screen), with
/// the keys held.
struct Invoke {
    hwnd: HWND,
    offset: usize,
    directory: Vec<u16>,
    at: POINT,
    shift: bool,
    ctrl: bool,
}

/// Carries out the menu's command `what.offset`, as File Explorer does. Its own failure is the
/// handler's to tell (it may have shown its own message), so none comes back.
fn invoke(menu: &IContextMenu, what: &Invoke) {
    let mut mask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
    if what.shift {
        mask |= CMIC_MASK_SHIFT_DOWN;
    }
    if what.ctrl {
        mask |= CMIC_MASK_CONTROL_DOWN;
    }
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: u32::try_from(size_of::<CMINVOKECOMMANDINFOEX>()).unwrap_or(u32::MAX),
        fMask: mask,
        hwnd: what.hwnd,
        // The command by its offset (`MAKEINTRESOURCE`), in both forms: a number, not memory.
        lpVerb: PCSTR(std::ptr::without_provenance(what.offset)),
        lpVerbW: PCWSTR(std::ptr::without_provenance(what.offset)),
        lpDirectoryW: PCWSTR(what.directory.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: what.at,
        ..Default::default()
    };
    // SAFETY: a whole `CMINVOKECOMMANDINFOEX` with its size set, which the call reads through
    // the `CMINVOKECOMMANDINFO` it begins with (`cbSize` tells it the rest); the folder's
    // NUL-terminated path lives through the call; the verbs are offsets below 0x10000, which the
    // handler takes as numbers.
    let _ = unsafe { menu.InvokeCommand((&raw const info).cast::<CMINVOKECOMMANDINFO>()) };
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

    /// The verbs of the menu's commands, each with its id, as the shell fills `popup`, through
    /// the submenus.
    fn verbs(menu: &IContextMenu, popup: HMENU) -> Vec<String> {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetMenuItemCount, GetMenuItemID, GetSubMenu,
        };
        let mut found = Vec::new();
        // SAFETY: a live menu of this thread.
        let count = unsafe { GetMenuItemCount(Some(popup)) };
        for place in 0..count {
            // SAFETY: as above, at a place it has.
            let id = unsafe { GetMenuItemID(popup, place) };
            if id == u32::MAX {
                // SAFETY: as above: a submenu, or a separator (none).
                let sub = unsafe { GetSubMenu(popup, place) };
                if !sub.is_invalid() {
                    found.extend(verbs(menu, sub));
                }
            } else if let Some(offset) = id
                .checked_sub(FIRST_COMMAND)
                .and_then(|offset| usize::try_from(offset).ok())
                && let Some(verb) = verb_of(menu, offset)
            {
                found.push(verb.to_lowercase());
            }
        }
        found
    }

    /// The verbs of the shell's menu of the items `names` of `folder`, filled as the app fills
    /// it (with its rename), without showing it.
    fn verbs_of(folder: &Path, names: &[&str]) -> Vec<String> {
        let names: Vec<OsString> = names.iter().map(OsString::from).collect();
        let menu = context_menu(HWND::default(), folder, &names).expect("the items' menu");
        // SAFETY: an empty menu, which `popup` destroys.
        let popup = PopupMenu(unsafe { CreatePopupMenu() }.expect("a menu"));
        // SAFETY: the empty menu, filled with the ids the app gives.
        unsafe {
            menu.QueryContextMenu(
                popup.0,
                0,
                FIRST_COMMAND,
                LAST_COMMAND,
                CMF_NORMAL | CMF_CANRENAME,
            )
        }
        .ok()
        .expect("the menu fills");
        verbs(&menu, popup.0)
    }

    #[test]
    fn the_shell_s_menu_of_a_file_has_the_commands_the_app_does_itself() {
        let _com = enter_apartment().expect("COM on the test's thread");
        let folder = tempfile::tempdir().expect("a folder");
        std::fs::write(folder.path().join("메뉴 (1).txt"), b"x").expect("a file");
        std::fs::create_dir(folder.path().join("하위")).expect("a folder");
        // One file: the app does these itself; the system the rest (properties among them).
        let found = verbs_of(folder.path(), &["메뉴 (1).txt"]);
        for verb in ["delete", "rename", "cut", "copy"] {
            assert!(found.iter().any(|found| found == verb), "{verb}: {found:?}");
            assert!(app_verb(verb, false).is_some(), "{verb}");
        }
        assert!(found.iter().any(|found| found == "properties"), "{found:?}");
        assert_eq!(app_verb("properties", false), None);
        // A file and a folder: one menu for both.
        let found = verbs_of(folder.path(), &["메뉴 (1).txt", "하위"]);
        for verb in ["delete", "cut", "copy", "properties"] {
            assert!(found.iter().any(|found| found == verb), "{verb}: {found:?}");
        }

        // The folder's own menu, as its part without items shows it.
        let background = context_menu(HWND::default(), folder.path(), &[]).expect("its menu");
        // SAFETY: as above.
        let popup = PopupMenu(unsafe { CreatePopupMenu() }.expect("a menu"));
        // SAFETY: as above.
        unsafe { background.QueryContextMenu(popup.0, 0, FIRST_COMMAND, LAST_COMMAND, CMF_NORMAL) }
            .ok()
            .expect("the menu fills");
        // A name that is not there is no item.
        let gone = [OsString::from("없는 파일.txt")];
        assert!(context_menu(HWND::default(), folder.path(), &gone).is_err());
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
