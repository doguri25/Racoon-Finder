#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! The demo app's executable. Its first argument is the folder the left pane starts in.

use std::path::PathBuf;
use std::process::ExitCode;

#[expect(
    clippy::print_stderr,
    reason = "a startup failure also goes to the terminal it was started from"
)]
fn main() -> ExitCode {
    let start = std::env::args_os().nth(1).map(PathBuf::from);
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
