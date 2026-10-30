//! macOS: the apps that can open a file and the one chosen to open it (`NSWorkspace`), Terminal
//! (`open -a Terminal`), and Finder's 정보 가져오기 window (`osascript`).
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

use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSError, NSFileManager, NSString, NSURL};

use super::{App, Launch, OpenWith, arrange_apps, info_window_launch, osascript_failure};

pub const OPEN_WITH: OpenWith = OpenWith::Apps;
pub const RUNS_AS_ADMIN: bool = false;

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

pub fn start_terminal(launches: &[Launch]) -> io::Result<()> {
    let Some(launch) = launches.first() else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no terminal program",
        ));
    };
    // `open` ends with a status of 1 when it finds no app named Terminal (the folder is there:
    // `open_terminal` looked).
    run(launch, |_| io::ErrorKind::Unsupported)
}

pub fn show_properties(path: &Path) -> io::Result<()> {
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

pub fn open_with_dialog(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "macOS lists the apps instead (apps_for)",
    ))
}

pub fn run_as_admin(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no program runs as administrator on macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
