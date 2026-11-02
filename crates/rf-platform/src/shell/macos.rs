#![allow(
    unsafe_code,
    reason = "the share picker's items are an untyped array of AppKit's, and the owner of a \
              window is made from the address of its view"
)]

//! macOS: the apps that can open a file and the one chosen to open it (`NSWorkspace`), Terminal
//! (`open -a Terminal`), Finder's 정보 가져오기 window (`osascript`), and the picker of the ways
//! to share items (`NSSharingServicePicker`, 공유…).
//!
//! `NSWorkspace` is not bound to the main thread (objc2-app-kit asks for no `MainThreadMarker`
//! for it, and Swift imports it without `@MainActor`), so these calls run on the caller's worker
//! thread, each inside an autorelease pool. `AppKit` calls the completion handler of an open on a
//! queue of its own; the handler only sends whether it worked over a channel, which the worker
//! waits on for a while.
//!
//! Finder's window comes from a fixed `AppleScript` run by `osascript`, which gets the path as its
//! one argument ([`info_window_launch`]). The first time, macOS asks the user whether the app may
//! control Finder (개인정보 보호 및 보안 ▸ 자동화; the bundle's `NSAppleEventsUsageDescription`
//! says why); a refusal comes back as `PermissionDenied`.
//!
//! The picker of 공유… is `AppKit`'s, on the main thread ([`share`]): the items' file URLs, byte
//! for byte as the clipboard writes them, by the point of the window's content view (winit's)
//! the menu showed at ([`view_point`]), below it. It shows its menu of services and returns;
//! `AppKit` keeps it until the user has chosen. There is no menu of the system's own of items
//! here (Windows' alone).

use std::ffi::{OsString, c_void};
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSRunningApplication, NSSharingServicePicker, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    NSArray, NSError, NSFileManager, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL,
};
use raw_window_handle::{RawWindowHandle, WindowHandle};

use super::{
    App, Handle, Launch, OpenWith, Owner, SystemMenuChoice, WindowPoint, arrange_apps,
    info_window_launch, open_failure, osascript_failure, view_point,
};
use crate::clipboard::file_url as exact_url;
use crate::dnd::content_view;

pub const OPEN_WITH: OpenWith = OpenWith::Apps;
pub const RUNS_AS_ADMIN: bool = false;
pub const SYSTEM_MENU: bool = false;
pub const SHARES: bool = true;

impl Owner {
    /// The window whose content is the macOS `NSView` at `view`.
    ///
    /// # Safety
    ///
    /// `view` must be the `NSView` of a window's content, and that window must live for as long
    /// as this owner, or a copy of it, is used: the drop target, a drag out and the picker of
    /// 공유… retain the view at that address and send it messages, on the main thread. A live
    /// window's handle gives one ([`Owner::of`]).
    pub unsafe fn view(view: NonNull<c_void>) -> Owner {
        Owner(Some(Handle::View(view.expose_provenance())))
    }
}

/// The owner of the window whose handle is `handle`, borrowed from the live window: the view of
/// its content for an `AppKit` handle ([`Owner::view`]), else [`Owner::NONE`].
pub(super) fn view_owner(handle: WindowHandle<'_>) -> Owner {
    match handle.as_raw() {
        // SAFETY: the handle borrows the live window and names its content view; the owner is
        // for use, on the main thread, while that window lives, as `Owner` says.
        RawWindowHandle::AppKit(appkit) => unsafe { Owner::view(appkit.ns_view) },
        _ => Owner::NONE,
    }
}

/// How long an open waits for the system to say that the app opened the file. An app that
/// takes longer to start is not a failure: the wait just ends.
const OPEN_WAIT: Duration = Duration::from_secs(30);

/// Runs `launch` and waits for it; a failure says what it wrote, through `failure`.
fn run(launch: &Launch, failure: impl Fn(&str) -> io::ErrorKind) -> io::Result<()> {
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(cwd) = &launch.cwd {
        command.current_dir(cwd);
    }
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    Err(io::Error::new(failure(&said), said.trim().to_owned()))
}

pub fn start_terminal(launches: &[Launch], _owner: Owner) -> io::Result<()> {
    let Some(launch) = launches.first() else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no terminal program",
        ));
    };
    // `open` says why it ends with a status of 1: no app named Terminal (`Unsupported`, as when
    // no terminal is there), or anything else, whose kind the notice says.
    run(launch, open_failure)
}

pub fn show_properties(path: &Path, _owner: Owner) -> io::Result<()> {
    run(&info_window_launch(path)?, osascript_failure)
}

/// `path` as the file URL of an `NSURL`.
fn file_url(path: &Path) -> io::Result<Retained<NSURL>> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a path of macOS is UTF-8"))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(text)))
}

/// The name the system shows for the app at `url`, and its path.
fn app_of(url: &NSURL, files: &NSFileManager) -> Option<(String, PathBuf)> {
    let path = url.path()?;
    let name = files.displayNameAtPath(&path).to_string();
    let path = PathBuf::from(OsString::from_vec(path.to_string().into_bytes()));
    Some((name, path))
}

pub fn apps_for(path: &Path) -> io::Result<Vec<App>> {
    autoreleasepool(|_| {
        let url = file_url(path)?;
        let workspace = NSWorkspace::sharedWorkspace();
        let files = NSFileManager::defaultManager();
        let found = workspace
            .URLsForApplicationsToOpenURL(&url)
            .to_vec()
            .iter()
            .filter_map(|app| app_of(app, &files))
            .collect();
        let default = workspace
            .URLForApplicationToOpenURL(&url)
            .and_then(|app| app_of(&app, &files));
        Ok(arrange_apps(found, default))
    })
}

pub fn open_with_app(path: &Path, app: &Path) -> io::Result<()> {
    let (sent, opened) = mpsc::channel::<bool>();
    autoreleasepool(|_| -> io::Result<()> {
        let urls = NSArray::from_retained_slice(&[file_url(path)?]);
        let app = file_url(app)?;
        let configuration = NSWorkspaceOpenConfiguration::configuration();
        // Called once, on a queue of AppKit's: it only sends on the channel, whose sender may be
        // used from any thread.
        let done = RcBlock::new(
            move |_running: *mut NSRunningApplication, error: *mut NSError| {
                let _ = sent.send(error.is_null());
            },
        );
        NSWorkspace::sharedWorkspace()
            .openURLs_withApplicationAtURL_configuration_completionHandler(
                &urls,
                &app,
                &configuration,
                Some(&done),
            );
        Ok(())
    })?;
    match opened.recv_timeout(OPEN_WAIT) {
        Ok(false) => Err(io::Error::other("the app did not open the file")),
        // It opened, it takes its time, or AppKit let the handler go uncalled: nothing to say.
        Ok(true) | Err(_) => Ok(()),
    }
}

pub fn open_with_dialog(_path: &Path, _owner: Owner) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "macOS lists the apps instead (apps_for)",
    ))
}

pub fn run_as_admin(_path: &Path, _owner: Owner) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no program runs as administrator on macOS",
    ))
}

pub fn system_menu(
    _owner: Owner,
    _folder: &Path,
    _names: &[OsString],
    _at: WindowPoint,
    _extended: bool,
) -> io::Result<SystemMenuChoice> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the menu of items is Windows' own",
    ))
}

pub fn share(owner: Owner, paths: &[PathBuf], at: WindowPoint) -> io::Result<()> {
    let (_main, view) = content_view(owner)?;
    autoreleasepool(|_| {
        let items = paths
            .iter()
            .map(|path| exact_url(path).map(|url| url.into_super().into_super()))
            .collect::<io::Result<Vec<Retained<AnyObject>>>>()?;
        let items = NSArray::from_retained_slice(&items);
        // SAFETY: the items are file URLs (`NSURL`), which the picker takes.
        let picker = unsafe {
            NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items)
        };
        let flipped = view.isFlipped();
        let (x, y) = view_point(at, view.bounds().size.height, flipped);
        // Below the point, as a menu opens there.
        let below = if flipped {
            NSRectEdge::MaxY
        } else {
            NSRectEdge::MinY
        };
        picker.showRelativeToRect_ofView_preferredEdge(
            NSRect::new(NSPoint::new(x, y), NSSize::new(1.0, 1.0)),
            &view,
            below,
        );
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use raw_window_handle::{AppKitWindowHandle, HandleError, HasWindowHandle};

    use super::*;

    /// A window whose handle names the view at the address it holds.
    struct Window(NonNull<c_void>);

    impl HasWindowHandle for Window {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
            // SAFETY: the handle lives no longer than this window, whose view is never sent a
            // message: the test only reads its address back.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    #[test]
    fn a_window_is_named_by_the_view_of_its_content() {
        let mut content = 0u8;
        let view = NonNull::from(&mut content).cast::<c_void>();
        let owner = Owner::of(&Window(view));
        assert_eq!(owner.ns_view(), Some(view));
        assert_eq!(owner.raw(), None);
        // SAFETY: as above: never used as a view.
        assert_eq!(unsafe { Owner::view(view) }, owner);
        assert_ne!(owner, Owner::NONE);
    }

    // Reads LaunchServices' database, which a logged-in session has; it shows nothing.
    #[test]
    fn the_apps_for_a_text_file_are_bundles() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let file = dir.path().join("메모.txt");
        std::fs::write(&file, "x").expect("a text file");
        let apps = apps_for(&file).expect("the apps");
        assert!(apps.iter().filter(|app| app.default).count() <= 1);
        assert!(apps.iter().skip(1).all(|app| !app.default));
        for app in &apps {
            assert!(app.path.is_absolute(), "{app:?}");
            assert!(!app.name.is_empty(), "{app:?}");
        }
    }
}
