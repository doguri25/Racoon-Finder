#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line example"
)]
//! A command-line front end of the job engine, to try it on real folders and drives.
//!
//! ```text
//! jobs [--journal <dir>] [--conflict ask|overwrite|skip|newer|size|rename] [--follow] <command>
//! ```
//!
//! Commands:
//!
//! - `copy <dest-dir> <source>...` and `move <dest-dir> <source>...`
//! - `trash <path>...`, which sends the paths to the trash, and `delete <path>...`, which
//!   deletes them for good
//! - `trash-undo <path>`, which trashes the path, waits for Enter and then undoes it
//! - `recover`, which only prints what the journal found of the jobs a crash cut off, and the
//!   temporary files removed
//!
//! The journal folder defaults to `jobs-example` in the temporary folder, named with the app's
//! temporary-file prefix. `--conflict` is what a transfer does with a name that is taken, `ask`
//! by default, which asks on the standard input; `--follow` copies what a link points at
//! instead of the link.
//!
//! The engine's events are printed as they come (`planned`, `progress`, `recovered`,
//! `temp_files_removed`), the job's log is written to the standard output when it ends, and the
//! exit code is 0 when every job completed, 1 when one did not and 2 for a command line it
//! cannot read.

use std::fmt::Display;
use std::io::{self, BufRead};
use std::path::{self, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use rf_core::VPath;
use rf_core::identity::TEMP_PREFIX;
use rf_jobs::{
    Conflict, ConflictAction, ConflictAnswer, ConflictPolicy, DeleteMethod, DeleteSpec,
    EngineConfig, ErrorAction, ErrorAnswer, ErrorPolicy, Issue, ItemFailure, JobEngine, JobEvent,
    JobId, JobReport, JobSpec, NonInteractive, Outcome, PreflightAnswer, Resolver, SymlinkMode,
    TransferMode, TransferOptions, TransferSpec,
};
use rf_vfs::Vfs;

const USAGE: &str = "usage: jobs [--journal <dir>] [--conflict ask|overwrite|skip|newer|size|rename] [--follow] <command>
commands:
  copy <dest-dir> <source>...
  move <dest-dir> <source>...
  trash <path>...
  delete <path>...      (for good)
  trash-undo <path>     (trash, wait for Enter, undo)
  recover               (print what the journal recovered, then end)";

/// How long the engine gets to write its last journal records when the program ends.
const SHUTDOWN: Duration = Duration::from_secs(10);

/// What the command line asks for.
struct Options {
    journal: PathBuf,
    conflict: ConflictPolicy,
    follow: bool,
    command: Command,
}

enum Command {
    Transfer {
        mode: TransferMode,
        dest: VPath,
        sources: Vec<VPath>,
    },
    Delete {
        method: DeleteMethod,
        paths: Vec<VPath>,
    },
    TrashUndo(VPath),
    Recover,
}

fn main() -> ExitCode {
    match parse(std::env::args_os().skip(1)) {
        Ok(options) => {
            if run(options) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(message) => {
            eprintln!("jobs: {}\n{USAGE}", one_line(message));
            ExitCode::from(2)
        }
    }
}

/// Reads the command line, the program name left out.
fn parse(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Options, String> {
    let mut args = args.peekable();
    let mut journal = std::env::temp_dir().join(format!("{TEMP_PREFIX}jobs-example"));
    let mut conflict = ConflictPolicy::Ask;
    let mut follow = false;
    while let Some(flag) = args.peek().and_then(|arg| arg.to_str()) {
        if !flag.starts_with("--") {
            break;
        }
        match flag {
            "--journal" => {
                args.next();
                journal = args.next().ok_or("--journal needs a folder")?.into();
            }
            "--conflict" => {
                args.next();
                let name = args.next().ok_or("--conflict needs a policy")?;
                conflict = match name.to_str() {
                    Some("ask") => ConflictPolicy::Ask,
                    Some("overwrite") => ConflictPolicy::Overwrite,
                    Some("skip") => ConflictPolicy::Skip,
                    Some("newer") => ConflictPolicy::OverwriteIfNewer,
                    Some("size") => ConflictPolicy::OverwriteIfSizeDiffers,
                    Some("rename") => ConflictPolicy::AutoRename,
                    _ => {
                        return Err(format!(
                            "unknown conflict policy {}",
                            name.to_string_lossy()
                        ));
                    }
                };
            }
            "--follow" => {
                args.next();
                follow = true;
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    let name = args.next().ok_or("missing command")?;
    let mut paths = args.map(|arg| local(&arg)).collect::<Result<Vec<_>, _>>()?;
    let command = match name.to_str() {
        Some(word @ ("copy" | "move")) => {
            if paths.len() < 2 {
                return Err(format!("{word} needs a destination folder and a source"));
            }
            let dest = paths.remove(0);
            let mode = if word == "copy" {
                TransferMode::Copy
            } else {
                TransferMode::Move
            };
            Command::Transfer {
                mode,
                dest,
                sources: paths,
            }
        }
        Some(word @ ("trash" | "delete")) => {
            if paths.is_empty() {
                return Err(format!("{word} needs a path"));
            }
            let method = if word == "trash" {
                DeleteMethod::Trash
            } else {
                DeleteMethod::Permanent
            };
            Command::Delete { method, paths }
        }
        Some("trash-undo") => match <[VPath; 1]>::try_from(paths) {
            Ok([path]) => Command::TrashUndo(path),
            Err(_) => return Err("trash-undo needs exactly one path".to_owned()),
        },
        Some("recover") if paths.is_empty() => Command::Recover,
        Some("recover") => return Err("recover takes no arguments".to_owned()),
        _ => return Err(format!("unknown command {}", name.to_string_lossy())),
    };
    Ok(Options {
        journal,
        conflict,
        follow,
        command,
    })
}

/// The absolute path the argument names, relative to the current folder.
fn local(arg: &std::ffi::OsStr) -> Result<VPath, String> {
    let absolute =
        path::absolute(arg).map_err(|error| format!("{}: {error}", arg.to_string_lossy()))?;
    VPath::local(&absolute).map_err(|error| format!("{}: {error}", arg.to_string_lossy()))
}

/// Runs the command on an engine of its own; `true` when every job completed.
fn run(options: Options) -> bool {
    let Options {
        journal,
        conflict,
        follow,
        command,
    } = options;
    let resolver: Arc<dyn Resolver> = if conflict == ConflictPolicy::Ask {
        Arc::new(StdinResolver)
    } else {
        Arc::new(NonInteractive)
    };
    let config = EngineConfig::new(Vfs::with_local(), resolver).with_journal(journal);
    let (engine, events) = JobEngine::start(config);
    let transfer = TransferOptions {
        conflict,
        errors: ErrorPolicy::Continue,
        symlinks: if follow {
            SymlinkMode::Follow
        } else {
            SymlinkMode::CopyLink
        },
    };
    let ok = match command {
        Command::Transfer {
            mode,
            dest,
            sources,
        } => match TransferSpec::into_dir(mode, &sources, &dest, transfer) {
            Ok(spec) => finish(&engine, &events, JobSpec::Transfer(spec)),
            Err(error) => {
                eprintln!("jobs: {}", one_line(error));
                false
            }
        },
        Command::Delete { method, paths } => {
            let spec = DeleteSpec {
                paths,
                method,
                errors: ErrorPolicy::Continue,
            };
            finish(&engine, &events, JobSpec::Delete(spec))
        }
        Command::TrashUndo(path) => trash_undo(&engine, &events, path),
        Command::Recover => {
            wait_for_recovery(&events);
            true
        }
    };
    // Lets the journal write its last records, so that the jobs that ended are not recovered.
    if !engine.shutdown(SHUTDOWN) {
        eprintln!("jobs: the engine did not stop in time");
    }
    ok
}

/// Trashes `path`, waits for Enter and undoes the trash.
fn trash_undo(engine: &JobEngine, events: &Receiver<JobEvent>, path: VPath) -> bool {
    let spec = DeleteSpec {
        paths: vec![path],
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    };
    if !finish(engine, events, JobSpec::Delete(spec)) {
        return false;
    }
    println!("press Enter to undo");
    let mut line = String::new();
    // Whatever ends the wait, the end of the input included, undoes.
    let _ = io::stdin().lock().read_line(&mut line);
    let Some(id) = engine.undo() else {
        eprintln!("jobs: nothing to undo");
        return false;
    };
    report_of(events, id).is_some_and(|report| log(&report))
}

/// Submits `spec`, prints the events until it ends, writes its log and says whether it
/// completed.
fn finish(engine: &JobEngine, events: &Receiver<JobEvent>, spec: JobSpec) -> bool {
    let id = engine.submit(spec);
    report_of(events, id).is_some_and(|report| log(&report))
}

/// Writes the log of `report` to the standard output; `true` when the job completed.
fn log(report: &JobReport) -> bool {
    if let Err(error) = report.write_log(&mut io::stdout().lock()) {
        eprintln!("jobs: cannot write the log: {}", one_line(error));
    }
    report.outcome == Outcome::Completed
}

/// Prints the events until the job `id` ends, and gives its report. `None` if the engine ends
/// first.
fn report_of(events: &Receiver<JobEvent>, id: JobId) -> Option<Arc<JobReport>> {
    while let Ok(event) = events.recv() {
        match event {
            JobEvent::Finished { id: ended, report } if ended == id => return Some(report),
            event => show(&event),
        }
    }
    None
}

/// Prints the events until the journal has been recovered, which every engine with a journal
/// announces once.
fn wait_for_recovery(events: &Receiver<JobEvent>) {
    while let Ok(event) = events.recv() {
        show(&event);
        if matches!(event, JobEvent::Recovered { .. }) {
            return;
        }
    }
}

/// Prints what an engine event tells, if it tells anything worth a line.
fn show(event: &JobEvent) {
    match event {
        JobEvent::Planned { totals, issues, .. } => println!(
            "planned files={} dirs={} bytes={} issues={issues}",
            totals.files, totals.dirs, totals.bytes
        ),
        JobEvent::Progress { progress, .. } => {
            let eta = progress
                .eta
                .map_or_else(|| "?".to_owned(), |eta| eta.as_secs().to_string());
            let current = progress
                .current
                .as_ref()
                .map_or_else(|| "-".to_owned(), one_line);
            println!(
                "progress {}/{} {} B/s eta {eta} {current}",
                progress.done.bytes, progress.total.bytes, progress.bytes_per_sec
            );
        }
        JobEvent::Recovered {
            jobs,
            temp_files_removed,
        } => {
            for job in jobs {
                println!("recovered {} started={}", job.id, job.started);
            }
            println!("temp_files_removed={temp_files_removed}");
        }
        JobEvent::JournalError { message } => eprintln!("jobs: journal: {}", one_line(message)),
        _ => {}
    }
}

/// `text` for one line of output: a control character (a line break in a file name, say) is
/// written as an escape, as in the job's log, so it cannot end the line or forge another.
fn one_line(text: impl Display) -> String {
    let mut line = String::new();
    for c in text.to_string().chars() {
        if c.is_control() {
            line.extend(c.escape_debug());
        } else {
            line.push(c);
        }
    }
    line
}

/// Reads one answer from the standard input: a letter, and an `a` after it for "all". `None`
/// at the end of the input; a line that is neither is `'?'`.
fn answer() -> Option<(char, bool)> {
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut letters = line
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase());
    let letter = letters.next().unwrap_or('?');
    Some(match letters.collect::<String>().as_str() {
        "" => (letter, false),
        "a" => (letter, true),
        _ => ('?', false),
    })
}

/// Asks the person at the terminal: a conflict, and the problems found on the target. The end
/// of the input cancels, so a job that is not at a terminal never waits for ever.
struct StdinResolver;

impl Resolver for StdinResolver {
    fn conflict(&self, _job: JobId, conflict: &Conflict) -> ConflictAnswer {
        loop {
            println!(
                "conflict {}: incoming {} bytes, existing {} bytes. [o]verwrite [s]kip [r]ename [c]ancel (add a for all)",
                one_line(&conflict.target),
                conflict.incoming.size,
                conflict.existing.size
            );
            let Some((letter, all)) = answer() else {
                return ConflictAnswer::once(ConflictAction::Cancel);
            };
            let action = match letter {
                'o' => ConflictAction::Overwrite,
                's' => ConflictAction::Skip,
                'r' => ConflictAction::Rename,
                'c' => ConflictAction::Cancel,
                _ => continue,
            };
            return if all {
                ConflictAnswer::all(action)
            } else {
                ConflictAnswer::once(action)
            };
        }
    }

    fn error(&self, _job: JobId, _failure: &ItemFailure) -> ErrorAnswer {
        ErrorAnswer {
            action: ErrorAction::Skip,
            apply_to_all: false,
        }
    }

    fn preflight(&self, _job: JobId, issues: &[Issue]) -> PreflightAnswer {
        for issue in issues {
            println!("{}", issue.log_line());
        }
        loop {
            println!("[p]roceed [s]kip blocked [c]ancel");
            match answer() {
                Some(('p', false)) => return PreflightAnswer::Proceed,
                Some(('s', false)) => return PreflightAnswer::SkipBlocked,
                Some(('c', false)) | None => return PreflightAnswer::Cancel,
                Some(_) => {}
            }
        }
    }
}
