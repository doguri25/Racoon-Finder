#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line example"
)]

//! Lists a folder with the local provider and prints the names in the default sort order,
//! mixing folders and files.
//!
//! `cargo run -p rf-vfs --example sorted -- <folder>` checks the natural order by hand:
//! compare the output with the order Explorer or Finder shows for the same folder.

use std::process::ExitCode;

use rf_core::VPath;
use rf_vfs::{CancelToken, LocalProvider, Provider, SortSpec, sort_entries};

fn main() -> ExitCode {
    let Some(arg) = std::env::args_os().nth(1) else {
        eprintln!("usage: sorted <folder>");
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
    let mut entries = Vec::new();
    let listed = LocalProvider::new().list(&dir, &CancelToken::new(), &mut |batch| {
        entries.extend(batch);
    });
    if let Err(e) = listed {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    let spec = SortSpec {
        folders_first: false,
        ..SortSpec::default()
    };
    sort_entries(&mut entries, &spec);
    for entry in &entries {
        println!("{}", entry.name.to_string_lossy());
    }
    ExitCode::SUCCESS
}
