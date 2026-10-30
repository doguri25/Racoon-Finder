#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line example"
)]

//! Sends the given files and folders to the system trash and says where each one went.
//!
//! `cargo run -p rf-platform --example trash -- <path>...` checks the real trash by hand.

use std::process::ExitCode;

use rf_core::VPath;
use rf_platform::{move_to_trash, trash_supported};

fn main() -> ExitCode {
    let mut failed = false;
    for arg in std::env::args_os().skip(1) {
        let path = std::path::absolute(&arg)
            .map_err(|e| e.to_string())
            .and_then(|absolute| VPath::local(absolute).map_err(|e| e.to_string()));
        let path = match path {
            Ok(path) => path,
            Err(e) => {
                eprintln!("error: {e}");
                failed = true;
                continue;
            }
        };
        println!("supported: {}", trash_supported(&path));
        match move_to_trash(&path) {
            Ok(Some(location)) => println!("trashed: {}", location.display()),
            Ok(None) => println!("trashed: (location unknown)"),
            Err(e) => {
                eprintln!("error: {e}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
