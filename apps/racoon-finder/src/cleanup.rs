//! `--cleanup [--backup-settings <file>]` and `--cleanup-plan` (PRD INST-04): the
//! clean removal the uninstaller runs, with no window and no lock logic of the app.
//!
//! - `--cleanup-plan` prints what `--cleanup` would do and does nothing.
//! - `--cleanup` asks a running app to end (a `quit.request` file beside its instance lock,
//!   [`rf_ui::instance::ask_to_quit`]) and waits up to [`QUIT_WAIT`] for the lock to be free;
//!   copies `settings.toml` to the `--backup-settings` file if asked to; then
//!   [`rf_ledger::run_cleanup`] puts every recorded system change back and deletes what
//!   [`rf_core::AppPaths::owned_paths`] names, and nothing else.
//! - A portable install has no uninstaller and is not touched: deleting its folder is the
//!   removal.
//!
//! The exit code says how it went ([`Exit`]), the output says what was done, in Korean.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rf_core::{AppPaths, InstallMode};
use rf_ledger::{CleanupAction, CleanupReport, LedgerError, SystemBackend};
use rf_ui::{instance, settings, words};

/// How long `--cleanup` waits for a running app to end: the app takes the request within a
/// moment, but ends only once its jobs have, or the user has answered its question.
pub const QUIT_WAIT: Duration = Duration::from_secs(30);

/// How many names a backup file tries before it gives up.
const BACKUP_TRIES: u32 = 1000;

/// How a run of `--cleanup` or `--cleanup-plan` ended, as the process's exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    /// Everything is done, or nothing was asked of a portable install, or the plan was printed.
    Done = 0,
    /// The folders could not be located, the lock could not be taken, or the ledger file is
    /// refused: nothing was deleted.
    Failed = 1,
    /// The arguments are not understood.
    Usage = 2,
    /// The app still runs after [`QUIT_WAIT`]: nothing was changed.
    StillRunning = 3,
    /// The settings could not be copied to the backup: nothing was deleted.
    BackupFailed = 4,
    /// Some of it remains: the report lists what.
    Partial = 5,
}

impl Exit {
    /// The process's exit code.
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `--cleanup`, with the file `--backup-settings` names.
    Cleanup { backup: Option<PathBuf> },
    /// `--cleanup-plan`, with the same backup, which the plan lists as its first step.
    Plan { backup: Option<PathBuf> },
}

/// The arguments are not `--cleanup` or `--cleanup-plan` followed by nothing or by
/// `--backup-settings <file>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage;

/// Reads the arguments after the program's name. `Ok(None)` when the first one is neither
/// `--cleanup` nor `--cleanup-plan`: the app starts as usual (its first argument is a folder).
pub fn parse(args: &[OsString]) -> Result<Option<Command>, Usage> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(None);
    };
    let cleanup = if first == "--cleanup" {
        true
    } else if first == "--cleanup-plan" {
        false
    } else {
        return Ok(None);
    };
    let mut backup = None;
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        if argument != "--backup-settings" || backup.is_some() {
            return Err(Usage);
        }
        match rest.next() {
            Some(file) if !file.is_empty() => backup = Some(PathBuf::from(file)),
            _ => return Err(Usage),
        }
    }
    Ok(Some(if cleanup {
        Command::Cleanup { backup }
    } else {
        Command::Plan { backup }
    }))
}

/// One step of a clean removal, in the order it runs ([`plan_with_backup`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Copy the settings file `from` to `to` (a name taken gets `-2`, `-3`, … before its
    /// extension).
    BackupSettings { from: PathBuf, to: PathBuf },
    /// A step of [`rf_ledger::plan_cleanup`].
    Ledger(CleanupAction),
}

/// What `--cleanup` does, in order: the copy of the settings first, when `backup` asks for one,
/// then the steps of [`rf_ledger::plan_cleanup`] (put the recorded changes back, delete what the
/// app owns). Nothing for a portable install, which has no uninstaller. Reads the ledger file
/// and folder listings only.
pub fn plan_with_backup(
    paths: &AppPaths,
    backup: Option<PathBuf>,
) -> Result<Vec<Step>, LedgerError> {
    if paths.mode() == InstallMode::Portable {
        return Ok(Vec::new());
    }
    let mut steps = Vec::new();
    if let Some(to) = backup {
        steps.push(Step::BackupSettings {
            from: paths.config_dir().join(settings::FILE_NAME),
            to,
        });
    }
    steps.extend(
        rf_ledger::plan_cleanup(paths)?
            .into_iter()
            .map(Step::Ledger),
    );
    Ok(steps)
}

/// Runs `command` for the app on this machine, writing what it says to `out`.
pub fn run(command: Command, out: &mut dyn Write) -> Exit {
    let paths = match AppPaths::detect() {
        Ok(paths) => paths,
        Err(error) => {
            say(out, &words::no_app_folders(error.kind()));
            return Exit::Failed;
        }
    };
    match command {
        Command::Plan { backup } => print_plan(&paths, backup, out),
        Command::Cleanup { backup } => {
            cleanup(&paths, backup, rf_ledger::os_backend(), QUIT_WAIT, out)
        }
    }
}

/// `--cleanup-plan`: lists the steps and does nothing.
pub fn print_plan(paths: &AppPaths, backup: Option<PathBuf>, out: &mut dyn Write) -> Exit {
    if paths.mode() == InstallMode::Portable {
        say(out, words::cleanup_portable());
        return Exit::Done;
    }
    match plan_with_backup(paths, backup) {
        Ok(steps) if steps.is_empty() => say(out, words::cleanup_plan_empty()),
        Ok(steps) => {
            for step in &steps {
                say(out, &step_line(step));
            }
        }
        Err(error) => {
            say(out, &words::cleanup_failed(&error.to_string()));
            return Exit::Failed;
        }
    }
    Exit::Done
}

/// `--cleanup`: ends a running app, backs the settings up if asked to, then cleans up with
/// `backend`, which the tests make a fake one. Waits up to `wait` for a running app to end.
pub fn cleanup(
    paths: &AppPaths,
    backup: Option<PathBuf>,
    backend: Box<dyn SystemBackend>,
    wait: Duration,
    out: &mut dyn Write,
) -> Exit {
    if paths.mode() == InstallMode::Portable {
        say(out, words::cleanup_portable());
        return Exit::Done;
    }
    // The app that runs is asked to end, and the lock of its job journal says when it has. A
    // data folder that is not there has no app running from it.
    let lock = if paths.data_dir().is_dir() {
        let ask = |dir: &Path| {
            instance::ask_to_quit(dir, instance::SHOW_WAIT);
        };
        match instance::take_after_quit(paths.data_dir(), ask, wait) {
            Ok(instance::Quit::Lock(lock)) => Some(lock),
            Ok(instance::Quit::Running) => {
                say(out, words::cleanup_still_running());
                return Exit::StillRunning;
            }
            Err(error) => {
                say(out, &words::cleanup_failed(&error.to_string()));
                return Exit::Failed;
            }
        }
    } else {
        None
    };
    if let Some(to) = backup {
        let from = paths.config_dir().join(settings::FILE_NAME);
        match back_up(&from, &to, paths.owned_paths()) {
            Ok(Some(copy)) => say(out, &words::cleanup_backup_done(&copy)),
            Ok(None) => say(out, words::cleanup_backup_none()),
            Err(error) => {
                // Nothing has been deleted: the settings are still where they were.
                say(out, &words::cleanup_backup_failed(&to, &error.to_string()));
                return Exit::BackupFailed;
            }
        }
    }
    // The lock file is in a folder that is about to go, and Windows cannot delete an open file.
    drop(lock);
    match rf_ledger::run_cleanup(paths, backend) {
        Ok(report) => finish(&report, out),
        Err(error) => {
            say(out, &words::cleanup_failed(&error.to_string()));
            Exit::Failed
        }
    }
}

/// Says how the cleanup went; `Done` when it is complete, `Partial` when something remains.
fn finish(report: &CleanupReport, out: &mut dyn Write) -> Exit {
    write_report(report, out);
    if report.is_complete() {
        Exit::Done
    } else {
        Exit::Partial
    }
}

/// Copies the settings file `from` to `to`, or to the first free one of `to`, `to-2`, `to-3`, …
/// (before the extension): a file that is there is never written over. `Ok(None)` when there is
/// no settings file to copy. The copy is refused when it would go inside a folder the cleanup
/// deletes (`owned`), and a copy that could not be finished is taken away again.
pub fn back_up(from: &Path, to: &Path, owned: &[PathBuf]) -> io::Result<Option<PathBuf>> {
    let mut source = match File::open(from) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let to = std::path::absolute(to)?;
    if owned.iter().any(|folder| to.starts_with(folder)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "inside the folders the cleanup deletes",
        ));
    }
    for number in 1..=BACKUP_TRIES {
        let path = numbered(&to, number);
        let mut copy = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let copied = io::copy(&mut source, &mut copy).and_then(|_| copy.sync_all());
        drop(copy);
        return match copied {
            Ok(()) => Ok(Some(path)),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(error)
            }
        };
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every name for the backup is taken",
    ))
}

/// `path` for 1, `stem-2.ext` for 2, and so on.
fn numbered(path: &Path, number: u32) -> PathBuf {
    if number <= 1 {
        return path.to_path_buf();
    }
    let mut name = path.file_stem().map(OsString::from).unwrap_or_default();
    name.push(format!("-{number}"));
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    path.with_file_name(name)
}

fn step_line(step: &Step) -> String {
    match step {
        Step::BackupSettings { from, to } => words::cleanup_step_backup(from, to),
        Step::Ledger(action) => action_line(action),
    }
}

fn action_line(action: &CleanupAction) -> String {
    match action {
        CleanupAction::Revert { change, .. } => words::cleanup_step_revert(change),
        CleanupAction::ClearDomain(domain) => words::cleanup_step_clear_domain(domain),
        CleanupAction::Delete(path) => words::cleanup_step_delete(path),
        CleanupAction::RemoveIfEmpty(path) => words::cleanup_step_remove_if_empty(path),
        CleanupAction::ListTemp(path) => words::cleanup_step_list_temp(path),
        _ => words::cleanup_step_other().to_owned(),
    }
}

fn write_report(report: &CleanupReport, out: &mut dyn Write) {
    if !report.reverted.reverted.is_empty() {
        say(
            out,
            &words::cleanup_reverted(report.reverted.reverted.len()),
        );
    }
    for failure in &report.reverted.failed {
        say(
            out,
            &words::cleanup_revert_failed(&failure.change, &failure.error),
        );
    }
    if report.deletes_skipped {
        say(out, words::cleanup_deletes_skipped());
    }
    for action in &report.done {
        say(out, &words::cleanup_done_line(&action_line(action)));
    }
    for (action, error) in &report.failed {
        say(
            out,
            &words::cleanup_remaining_line(&action_line(action), error),
        );
    }
    say(
        out,
        if report.is_complete() {
            words::cleanup_complete()
        } else {
            words::cleanup_partial()
        },
    );
}

/// Writes a line. A closed output (no console, as in a Windows release build) is no failure.
fn say(out: &mut dyn Write, line: &str) {
    let _ = writeln!(out, "{line}");
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use rf_core::{BaseDirs, Os};
    use rf_ledger::FakeBackend;

    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// The layout of this system under `root`: no `portable` marker next to the executable, so
    /// it is an installed one. Nothing is read from or written to the real home.
    fn installed_under(root: &Path) -> AppPaths {
        let exe = root.join("bin").join("app");
        std::fs::create_dir_all(exe.parent().expect("a folder")).expect("made");
        AppPaths::resolve(Os::CURRENT, &exe, &base_dirs(root)).expect("the layout")
    }

    /// The base folders of every system, all under `root`.
    fn base_dirs(root: &Path) -> BaseDirs {
        let home = root.join("home");
        BaseDirs {
            app_data: Some(home.join("AppData").join("Roaming")),
            local_app_data: Some(home.join("AppData").join("Local")),
            home: Some(home),
            temp: root.join("tmp"),
        }
    }

    /// A portable layout: the marker is next to the executable.
    fn portable_under(root: &Path) -> AppPaths {
        let exe = root.join("portable").join("app");
        std::fs::create_dir_all(exe.parent().expect("a folder")).expect("made");
        std::fs::write(exe.with_file_name(rf_core::identity::PORTABLE_MARKER), b"")
            .expect("marker");
        AppPaths::resolve(Os::CURRENT, &exe, &base_dirs(root)).expect("the layout")
    }

    fn text(out: &[u8]) -> String {
        String::from_utf8(out.to_vec()).expect("text")
    }

    #[test]
    fn the_command_line_is_read() {
        assert_eq!(parse(&[]), Ok(None));
        // A folder to start in is no cleanup.
        assert_eq!(parse(&args(&["/tmp"])), Ok(None));
        assert_eq!(
            parse(&args(&["--cleanup"])),
            Ok(Some(Command::Cleanup { backup: None }))
        );
        assert_eq!(
            parse(&args(&["--cleanup-plan"])),
            Ok(Some(Command::Plan { backup: None }))
        );
        assert_eq!(
            parse(&args(&["--cleanup", "--backup-settings", "b.toml"])),
            Ok(Some(Command::Cleanup {
                backup: Some(PathBuf::from("b.toml"))
            }))
        );
        assert_eq!(
            parse(&args(&["--cleanup-plan", "--backup-settings", "b.toml"])),
            Ok(Some(Command::Plan {
                backup: Some(PathBuf::from("b.toml"))
            }))
        );
        for odd in [
            &["--cleanup", "--backup-settings"][..],
            &["--cleanup", "--backup-settings", ""],
            &[
                "--cleanup",
                "--backup-settings",
                "a",
                "--backup-settings",
                "b",
            ],
            &["--cleanup", "extra"],
            &["--cleanup-plan", "--backup-settings", "a", "extra"],
            &["--cleanup", "--other", "a"],
        ] {
            assert_eq!(parse(&args(odd)), Err(Usage), "{odd:?}");
        }
    }

    #[test]
    fn the_backup_comes_before_every_delete_and_a_portable_install_has_no_steps() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        let to = root.path().join("backup.toml");

        let steps = plan_with_backup(&paths, Some(to.clone())).expect("a plan");

        assert_eq!(
            steps.first(),
            Some(&Step::BackupSettings {
                from: paths.config_dir().join(settings::FILE_NAME),
                to,
            })
        );
        assert!(
            steps[1..]
                .iter()
                .all(|step| matches!(step, Step::Ledger(_))),
            "{steps:?}"
        );
        // After the backup come the deletes of exactly what the app owns.
        let deleted: Vec<&PathBuf> = steps
            .iter()
            .filter_map(|step| match step {
                Step::Ledger(CleanupAction::Delete(path)) => Some(path),
                _ => None,
            })
            .collect();
        assert_ne!(deleted.len(), 0);
        assert!(
            deleted.iter().all(|path| paths
                .owned_paths()
                .iter()
                .any(|owned| path.starts_with(owned))),
            "{deleted:?}"
        );
        // Without a backup the plan is the ledger's own.
        let plain = plan_with_backup(&paths, None).expect("a plan");
        assert_eq!(plain, steps[1..]);

        // A portable install has no uninstaller: no steps, with or without a backup.
        let portable = portable_under(root.path());
        assert_eq!(portable.mode(), InstallMode::Portable);
        assert_eq!(plan_with_backup(&portable, None).expect("a plan"), []);
        assert_eq!(
            plan_with_backup(&portable, Some(root.path().join("b.toml"))).expect("a plan"),
            []
        );
    }

    #[test]
    fn the_plan_lists_the_steps_and_changes_nothing() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        std::fs::write(
            paths.config_dir().join(settings::FILE_NAME),
            "theme = \"dark\"\n",
        )
        .expect("settings");
        let to = root.path().join("backup.toml");
        let mut out = Vec::new();

        let exit = print_plan(&paths, Some(to.clone()), &mut out);

        assert_eq!(exit, Exit::Done);
        let printed = text(&out);
        let lines: Vec<&str> = printed.lines().collect();
        assert!(lines[0].contains(&to.display().to_string()), "{printed}");
        for owned in paths.owned_paths() {
            assert!(
                printed.contains(&owned.display().to_string()),
                "{owned:?} in {printed}"
            );
            assert!(owned.exists(), "{owned:?} is still there");
        }
        assert!(!to.exists());
        assert!(paths.config_dir().join(settings::FILE_NAME).is_file());

        // A portable install says so and lists nothing.
        let mut out = Vec::new();
        let portable = portable_under(root.path());
        assert_eq!(print_plan(&portable, None, &mut out), Exit::Done);
        assert_eq!(text(&out), "포터블 설치는 폴더를 지우면 됩니다.\n");
    }

    /// Fills every folder the app owns, and puts a folder of another app beside each, a file
    /// of the user's in the home folder, and a file of someone else in the shared temp folder.
    fn fill(paths: &AppPaths, root: &Path) -> Vec<PathBuf> {
        paths.ensure_dirs().expect("folders");
        std::fs::write(
            paths.config_dir().join(settings::FILE_NAME),
            "theme = \"dark\"\n",
        )
        .expect("settings");
        std::fs::create_dir_all(paths.cache_dir().join("thumbs").join("256")).expect("thumbs");
        std::fs::write(
            paths.cache_dir().join("thumbs").join("256").join("a.png"),
            b"png",
        )
        .expect("a thumbnail");
        std::fs::write(paths.log_dir().join("job.log"), b"log").expect("a log");
        std::fs::write(paths.data_dir().join("journal"), b"journal").expect("a journal");
        std::fs::write(
            paths
                .temp_dir()
                .join(format!("{}stale", rf_core::identity::TEMP_PREFIX)),
            b"temp",
        )
        .expect("a temp file");
        let mut kept = Vec::new();
        let others = std::iter::once(root.join("home").join("Documents")).chain(
            paths
                .owned_paths()
                .iter()
                .filter_map(|owned| owned.parent().map(|dir| dir.join("OtherApp"))),
        );
        for folder in others {
            std::fs::create_dir_all(&folder).expect("a folder of another app");
            let file = folder.join("keep.txt");
            std::fs::write(&file, b"keep").expect("a file to keep");
            kept.push(file);
        }
        let neighbour = paths.temp_dir().join("someone-else.tmp");
        std::fs::write(&neighbour, b"keep").expect("a temp file of another");
        kept.push(neighbour);
        kept
    }

    #[test]
    fn a_cleanup_deletes_what_the_app_owns_and_leaves_one_backup_and_the_rest() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        let kept = fill(&paths, root.path());
        let documents = root.path().join("home").join("Documents");
        let to = documents.join("settings-backup.toml");
        let mut out = Vec::new();

        let exit = cleanup(
            &paths,
            Some(to.clone()),
            Box::new(FakeBackend::new()),
            Duration::from_secs(5),
            &mut out,
        );

        assert_eq!(exit, Exit::Done, "{}", text(&out));
        // What the app owns is gone, the thumbnails with it.
        for owned in paths.owned_paths() {
            assert!(!owned.exists(), "{owned:?} is deleted");
        }
        assert!(!paths.cache_dir().join("thumbs").exists());
        assert!(
            !paths
                .temp_dir()
                .join(format!("{}stale", rf_core::identity::TEMP_PREFIX))
                .exists()
        );
        // Everything else stays.
        for file in &kept {
            assert_eq!(std::fs::read(file).expect("kept"), b"keep", "{file:?}");
        }
        // Exactly one backup file appeared, with the settings in it.
        assert_eq!(
            std::fs::read_to_string(&to).expect("backup"),
            "theme = \"dark\"\n"
        );
        let names: Vec<String> = std::fs::read_dir(&documents)
            .expect("documents")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let mut backups: Vec<&String> = names
            .iter()
            .filter(|name| name.contains("settings-backup"))
            .collect();
        backups.sort();
        assert_eq!(backups, ["settings-backup.toml"]);
        assert!(
            text(&out).ends_with("정리를 마쳤습니다.\n"),
            "{}",
            text(&out)
        );
    }

    #[test]
    fn a_backup_that_fails_deletes_nothing() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        let kept = fill(&paths, root.path());
        // The folder for the backup is not there, and a cleanup does not make folders.
        let to = root.path().join("no such folder").join("backup.toml");
        let mut out = Vec::new();

        let exit = cleanup(
            &paths,
            Some(to),
            Box::new(FakeBackend::new()),
            Duration::from_secs(5),
            &mut out,
        );

        assert_eq!(exit, Exit::BackupFailed, "{}", text(&out));
        for owned in paths.owned_paths() {
            assert!(owned.exists(), "{owned:?} stays");
        }
        assert!(paths.config_dir().join(settings::FILE_NAME).is_file());
        for file in &kept {
            assert!(file.is_file(), "{file:?}");
        }
        // A backup inside what is about to be deleted is refused the same way.
        let inside = paths.config_dir().join("backup.toml");
        let exit = cleanup(
            &paths,
            Some(inside.clone()),
            Box::new(FakeBackend::new()),
            Duration::from_secs(5),
            &mut Vec::new(),
        );
        assert_eq!(exit, Exit::BackupFailed);
        assert!(!inside.exists());
        assert!(paths.config_dir().join(settings::FILE_NAME).is_file());
    }

    #[test]
    fn a_backup_never_takes_the_place_of_a_file_that_is_there() {
        let root = tempfile::tempdir().expect("a temp dir");
        let from = root.path().join("settings.toml");
        std::fs::write(&from, "theme = \"light\"\n").expect("settings");
        let to = root.path().join("backup.toml");
        std::fs::write(&to, "an older backup").expect("an older backup");
        std::fs::write(root.path().join("backup-2.toml"), "another").expect("another");

        let copy = back_up(&from, &to, &[]).expect("copied").expect("a copy");

        assert_eq!(copy, root.path().join("backup-3.toml"));
        assert_eq!(
            std::fs::read_to_string(&to).expect("kept"),
            "an older backup"
        );
        assert_eq!(
            std::fs::read_to_string(&copy).expect("copy"),
            "theme = \"light\"\n"
        );
        // No settings, nothing to copy.
        assert_eq!(
            back_up(&root.path().join("missing.toml"), &to, &[]).expect("no error"),
            None
        );
    }

    #[test]
    fn a_portable_install_is_left_alone() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = portable_under(root.path());
        paths.ensure_dirs().expect("folders");
        std::fs::write(paths.config_dir().join(settings::FILE_NAME), "x").expect("settings");
        let mut out = Vec::new();

        let exit = cleanup(
            &paths,
            Some(root.path().join("backup.toml")),
            Box::new(FakeBackend::new()),
            Duration::from_secs(1),
            &mut out,
        );

        assert_eq!(exit, Exit::Done);
        assert_eq!(text(&out), "포터블 설치는 폴더를 지우면 됩니다.\n");
        assert!(paths.config_dir().join(settings::FILE_NAME).is_file());
        assert!(!root.path().join("backup.toml").exists());
    }

    #[test]
    fn a_running_app_is_asked_to_end_and_the_cleanup_goes_on_once_it_has() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        let kept = fill(&paths, root.path());
        // The running app: it holds the lock, and ends (lets the lock go) a moment after it
        // takes the request to quit.
        let running = instance::InstanceLock::acquire(paths.data_dir()).expect("the app's lock");
        let (quit, asked) = mpsc::channel::<()>();
        let watch = running
            .watch_requests(
                || {},
                move || {
                    let _ = quit.send(());
                },
            )
            .expect("the look starts");
        let ending = std::thread::spawn(move || {
            asked
                .recv_timeout(Duration::from_secs(10))
                .expect("asked to quit");
            std::thread::sleep(Duration::from_millis(300));
            drop(watch);
            drop(running);
        });
        let mut out = Vec::new();

        let exit = cleanup(
            &paths,
            None,
            Box::new(FakeBackend::new()),
            Duration::from_secs(20),
            &mut out,
        );

        ending.join().expect("the app ended");
        assert_eq!(exit, Exit::Done, "{}", text(&out));
        for owned in paths.owned_paths() {
            assert!(!owned.exists(), "{owned:?} is deleted");
        }
        for file in &kept {
            assert!(file.is_file(), "{file:?}");
        }
    }

    #[test]
    fn an_app_that_does_not_end_is_left_running_and_nothing_is_deleted() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        fill(&paths, root.path());
        // It takes the request (jobs run: it asks its user instead of ending) and keeps the lock.
        let running = instance::InstanceLock::acquire(paths.data_dir()).expect("the app's lock");
        let (quit, asked) = mpsc::channel::<()>();
        let watch = running
            .watch_requests(
                || {},
                move || {
                    let _ = quit.send(());
                },
            )
            .expect("the look starts");
        let mut out = Vec::new();

        let exit = cleanup(
            &paths,
            Some(root.path().join("backup.toml")),
            Box::new(FakeBackend::new()),
            Duration::from_secs(1),
            &mut out,
        );

        assert_eq!(exit, Exit::StillRunning);
        assert_eq!(
            text(&out),
            "앱이 아직 실행 중입니다. 앱을 끝낸 뒤 다시 시도하세요.\n"
        );
        asked.try_recv().expect("the app was asked to quit");
        for owned in paths.owned_paths() {
            assert!(owned.exists(), "{owned:?} stays");
        }
        assert!(!root.path().join("backup.toml").exists());
        drop(watch);
        drop(running);
    }

    #[test]
    fn what_remains_is_reported_and_exits_partial() {
        let mut report = CleanupReport::default();
        let mut out = Vec::new();
        assert_eq!(finish(&report, &mut out), Exit::Done);
        assert_eq!(text(&out), "정리를 마쳤습니다.\n");

        report
            .done
            .push(CleanupAction::Delete(PathBuf::from("done-folder")));
        report.failed.push((
            CleanupAction::Delete(PathBuf::from("left-folder")),
            "in use".to_owned(),
        ));
        let mut out = Vec::new();
        assert_eq!(finish(&report, &mut out), Exit::Partial);
        let printed = text(&out);
        assert!(printed.contains("done-folder"), "{printed}");
        assert!(
            printed.contains("left-folder") && printed.contains("in use"),
            "{printed}"
        );
        assert!(
            printed.ends_with("일부를 정리하지 못했습니다.\n"),
            "{printed}"
        );

        // A change that could not be put back keeps everything else from being deleted.
        let mut report = CleanupReport::default();
        report.deletes_skipped = true;
        let mut out = Vec::new();
        assert_eq!(finish(&report, &mut out), Exit::Partial);
        assert!(
            text(&out).contains("아무것도 지우지 않았습니다"),
            "{}",
            text(&out)
        );
    }

    #[test]
    fn the_exit_codes_are_the_ones_the_installer_reads() {
        assert_eq!(
            [
                Exit::Done,
                Exit::Failed,
                Exit::Usage,
                Exit::StillRunning,
                Exit::BackupFailed,
                Exit::Partial,
            ]
            .map(Exit::code),
            [0, 1, 2, 3, 4, 5]
        );
    }
}
