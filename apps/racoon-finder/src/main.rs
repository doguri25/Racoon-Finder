#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! The demo app's executable. Its first argument is the folder the left pane starts in.

use std::path::PathBuf;
use std::process::ExitCode;

#[expect(
    clippy::print_stderr,
    reason = "the only place a startup failure can still be reported"
)]
fn main() -> ExitCode {
    let start = std::env::args_os().nth(1).map(PathBuf::from);
    match rf_ui::run(start) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
