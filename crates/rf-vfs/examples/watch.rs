#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line example"
)]

//! Watches a folder and prints every change the local provider reports.
//!
//! `cargo run -p rf-vfs --example watch -- <folder>` checks live watching by hand: change the
//! folder from a file manager or a shell and see the events. Press Enter to stop.

use std::process::ExitCode;

use rf_core::VPath;
use rf_vfs::{LocalProvider, Provider};

fn main() -> ExitCode {
    let Some(arg) = std::env::args_os().nth(1) else {
        eprintln!("usage: watch <folder>");
        return ExitCode::FAILURE;
    };
    let dir = match std::path::absolute(&arg)
        .map_err(|e| e.to_string())
        .and_then(|absolute| VPath::local(absolute).map_err(|e| e.to_string()))
    {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let handle = match LocalProvider::new().watch(&dir, Box::new(|event| println!("{event:?}"))) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("watching {dir}; press Enter to stop");
    let _ = std::io::stdin().read_line(&mut String::new());
    drop(handle);
    ExitCode::SUCCESS
}
