#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! The demo app's executable. Its first argument is the folder the left pane starts in, except
//! for `--cleanup` and `--cleanup-plan`, which run the clean removal without a window
//! ([`cleanup`]).

mod cleanup;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

#[expect(
    clippy::print_stderr,
    reason = "a startup failure also goes to the terminal it was started from"
)]
fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    // Before any window or lock of the app: the clean removal ends a running app itself.
    match cleanup::parse(&args) {
        Ok(Some(command)) => {
            let exit = cleanup::run(command, &mut std::io::stdout().lock());
            return ExitCode::from(exit.code());
        }
        Ok(None) => {}
        Err(cleanup::Usage) => {
            eprintln!("{}", rf_ui::words::cleanup_usage());
            return ExitCode::from(cleanup::Exit::Usage.code());
        }
    }
    let start = args.into_iter().next().map(PathBuf::from);
    match rf_ui::run(start) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            // A release build on Windows has no console: the user sees why in a window.
            rf_ui::report_startup_failure(&error);
            ExitCode::FAILURE
        }
    }
}
