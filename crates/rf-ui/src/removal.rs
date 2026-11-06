//! Removing the app from the computer (PRD INST-04, INST-05): what the settings menu offers
//! at its bottom, and the steps of the complete removal (완전 삭제…) that need no window.
//!
//! - **macOS:** 완전 삭제… puts every recorded change of the system back, deletes what the app
//!   owns (Application Support, Caches, Logs, the preferences) and moves the app bundle to the
//!   trash ([`remove`]). Before any of it the settings can be copied to the documents folder
//!   ([`back_up_settings`]); nothing is deleted when that fails.
//! - **Windows, installed:** 제거 프로그램 열기 starts the uninstaller the installer put beside
//!   the executable ([`UNINSTALLER`]), which runs the same cleanup (`--cleanup`).
//! - **Windows portable, Linux:** nothing is offered. A portable app has no uninstaller (the
//!   folder is deleted), and Linux is for development.
//!
//! What is offered is a pure decision ([`offer`]), made once at the start ([`detect`]); the
//! steps are plain functions that the window runs on a worker thread and the tests run on a
//! fake system. The same [`back_up`] and [`report_lines`] serve `--cleanup`.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use rf_core::{AppPaths, InstallMode, Os};
use rf_ledger::{CleanupAction, CleanupReport, LedgerError, SystemBackend};

use crate::{settings, words};

/// The name of the uninstaller the Windows installer puts beside the executable.
pub const UNINSTALLER: &str = "unins000.exe";

/// The name of the file the settings are copied to in the documents folder.
pub fn backup_file_name() -> String {
    format!("{}-settings-backup.toml", rf_core::identity::DATA_DIR_NAME)
}

/// How many names a backup file tries before it gives up.
const BACKUP_TRIES: u32 = 1000;

/// What the bottom of 보기 ▸ 설정 offers for removing the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// the complete removal (macOS).
    DeleteCompletely,
    /// "제거 프로그램 열기" (an installed app on Windows).
    OpenUninstaller,
}

impl Offer {
    /// The number the window's `removal-offer` property takes for this offer (0 is none).
    pub fn number(offer: Option<Offer>) -> i32 {
        match offer {
            None => 0,
            Some(Offer::DeleteCompletely) => 1,
            Some(Offer::OpenUninstaller) => 2,
        }
    }
}

/// What the settings menu offers on `os` for an app installed as `mode`, when what removes the
/// app is there (`remover_present`: the app bundle on macOS, [`UNINSTALLER`] on Windows).
///
/// macOS offers 완전 삭제… when the program runs from an app bundle (a program run from a build
/// folder has none to move to the trash, and deleting its data alone would leave half a
/// removal). Windows offers 제거 프로그램 열기 only for an installed app: the `portable` marker
/// next to the executable means there is no uninstaller to open. Linux offers nothing.
pub fn offer(os: Os, mode: InstallMode, remover_present: bool) -> Option<Offer> {
    if !remover_present {
        return None;
    }
    match (os, mode) {
        (Os::MacOs, InstallMode::Installed) => Some(Offer::DeleteCompletely),
        (Os::Windows, InstallMode::Installed) => Some(Offer::OpenUninstaller),
        _ => None,
    }
}

/// The uninstaller beside the executable `exe`.
pub fn uninstaller_beside(exe: &Path) -> PathBuf {
    exe.with_file_name(UNINSTALLER)
}

/// What the settings menu offers for the app that runs as `exe` with the layout `paths`
/// ([`offer`]). Looks at the disk (the uninstaller on Windows), so it runs at the start, before
/// the window shows, and not later on the UI thread. `exe` is `None` when the system does not
/// say where the program is: then nothing is offered.
pub fn detect(paths: &AppPaths, exe: Option<&Path>) -> Option<Offer> {
    let exe = exe?;
    let present = match Os::CURRENT {
        Os::MacOs => rf_platform::app_bundle_of(exe).is_some(),
        Os::Windows => uninstaller_beside(exe).is_file(),
        Os::Other => false,
    };
    offer(Os::CURRENT, paths.mode(), present)
}

/// The file the settings are copied to when 설정 백업 is chosen: in the documents folder
/// `documents`.
pub fn backup_target(documents: &Path) -> PathBuf {
    documents.join(backup_file_name())
}

/// What 완전 삭제… will do, a line each, for the window to list: the steps of
/// [`rf_ledger::plan_cleanup`] (put the recorded changes back, delete what the app owns), then
/// moving the app at `bundle` to the trash. Reads the ledger file and folder listings only, so
/// it is for a worker thread.
pub fn plan_lines(paths: &AppPaths, bundle: Option<&Path>) -> Result<Vec<String>, LedgerError> {
    let mut lines: Vec<String> = rf_ledger::plan_cleanup(paths)?
        .iter()
        .map(action_line)
        .collect();
    lines.extend(bundle.map(words::removal_step_bundle));
    Ok(lines)
}

/// The words for one step of the clean removal.
pub fn action_line(action: &CleanupAction) -> String {
    match action {
        CleanupAction::Revert { change, .. } => words::cleanup_step_revert(change),
        CleanupAction::ClearDomain(domain) => words::cleanup_step_clear_domain(domain),
        CleanupAction::Delete(path) => words::cleanup_step_delete(path),
        CleanupAction::RemoveIfEmpty(path) => words::cleanup_step_remove_if_empty(path),
        CleanupAction::ListTemp(path) => words::cleanup_step_list_temp(path),
        _ => words::cleanup_step_other().to_owned(),
    }
}

/// What a finished [`rf_ledger::run_cleanup`] says, a line each: the changes put back and the
/// ones that could not be, what was done and what remains, and last whether it is complete.
pub fn report_lines(report: &CleanupReport) -> Vec<String> {
    let mut lines = Vec::new();
    if !report.reverted.reverted.is_empty() {
        lines.push(words::cleanup_reverted(report.reverted.reverted.len()));
    }
    for failure in &report.reverted.failed {
        lines.push(words::cleanup_revert_failed(
            &failure.change,
            &failure.error,
        ));
    }
    if report.deletes_skipped {
        lines.push(words::cleanup_deletes_skipped().to_owned());
    }
    for action in &report.done {
        lines.push(words::cleanup_done_line(&action_line(action)));
    }
    for (action, error) in &report.failed {
        lines.push(words::cleanup_remaining_line(&action_line(action), error));
    }
    lines.push(
        if report.is_complete() {
            words::cleanup_complete()
        } else {
            words::cleanup_partial()
        }
        .to_owned(),
    );
    lines
}

/// Copies the settings of the app to `to` (or the first free name after it, [`back_up`]).
/// `Ok(None)` when there are no settings to copy. Nothing is changed when it fails, and the
/// caller then stops: nothing is deleted before the backup is made.
pub fn back_up_settings(paths: &AppPaths, to: &Path) -> io::Result<Option<PathBuf>> {
    back_up(
        &paths.config_dir().join(settings::FILE_NAME),
        to,
        paths.owned_paths(),
    )
}

/// How far [`remove`] got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Everything is gone: the app's data, and the app itself is in the trash at the place.
    Done { trashed: PathBuf },
    /// The cleanup could not run or left something: nothing else was done (the app is not moved
    /// to the trash). `lines` say what is done and what remains.
    Incomplete { lines: Vec<String> },
    /// The app's data is gone, but the app could not be moved to the trash.
    NotTrashed { reason: String, lines: Vec<String> },
}

/// Puts every recorded change of the system back and deletes what the app owns
/// ([`rf_ledger::run_cleanup`] with `backend`), and only when that left nothing, moves the app
/// to the trash with `trash`. Blocks: for a worker thread. The app's own jobs, saves and
/// watches must have ended before, so that nothing writes into the folders being deleted.
pub fn remove(
    paths: &AppPaths,
    backend: Box<dyn SystemBackend>,
    trash: impl FnOnce() -> io::Result<PathBuf>,
) -> Outcome {
    let report = match rf_ledger::run_cleanup(paths, backend) {
        Ok(report) => report,
        Err(error) => {
            return Outcome::Incomplete {
                lines: vec![words::cleanup_failed(&error.to_string())],
            };
        }
    };
    let lines = report_lines(&report);
    if !report.is_complete() {
        return Outcome::Incomplete { lines };
    }
    match trash() {
        Ok(trashed) => Outcome::Done { trashed },
        Err(error) => Outcome::NotTrashed {
            reason: error.to_string(),
            lines,
        },
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
    let to = resolved_destination(to)?;
    if owned.iter().any(|folder| to.starts_with(resolved(folder))) {
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

/// `to` with its folder resolved (`..`, links, the case of a name on Windows): the folder must
/// be there, a cleanup does not make folders.
fn resolved_destination(to: &Path) -> io::Result<PathBuf> {
    let to = std::path::absolute(to)?;
    let (Some(folder), Some(name)) = (to.parent(), to.file_name()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not the name of a file",
        ));
    };
    Ok(std::fs::canonicalize(folder)?.join(name))
}

/// `path` as the file system names it. For a folder that is not there (nothing to delete in
/// it) the nearest folder above that is, with the rest of the name after it; where nothing
/// can be resolved, the path as it is.
fn resolved(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest = Vec::new();
    let mut above = path.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(above) {
            return rest.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (above.parent(), above.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                above = parent;
            }
            _ => return path,
        }
    }
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

#[cfg(test)]
mod tests {
    use rf_core::BaseDirs;
    use rf_ledger::FakeBackend;

    use super::*;

    /// The layout of this system under `root`: no `portable` marker next to the executable, so
    /// it is an installed one.
    fn installed_under(root: &Path) -> AppPaths {
        let exe = root.join("bin").join("app");
        std::fs::create_dir_all(exe.parent().expect("a folder")).expect("made");
        let home = root.join("home");
        let base = BaseDirs {
            app_data: Some(home.join("AppData").join("Roaming")),
            local_app_data: Some(home.join("AppData").join("Local")),
            home: Some(home),
            temp: root.join("tmp"),
        };
        AppPaths::resolve(Os::CURRENT, &exe, &base).expect("the layout")
    }

    #[test]
    fn the_menu_offers_a_removal_by_system_and_install() {
        use InstallMode::{Installed, Portable};
        // macOS: from a bundle, 완전 삭제….
        assert_eq!(
            offer(Os::MacOs, Installed, true),
            Some(Offer::DeleteCompletely)
        );
        assert_eq!(offer(Os::MacOs, Installed, false), None);
        // Windows: the uninstaller of an installed app, never of a portable one.
        assert_eq!(
            offer(Os::Windows, Installed, true),
            Some(Offer::OpenUninstaller)
        );
        assert_eq!(offer(Os::Windows, Installed, false), None);
        assert_eq!(offer(Os::Windows, Portable, true), None);
        assert_eq!(offer(Os::Windows, Portable, false), None);
        // Linux, which is for development, and a portable layout anywhere else.
        assert_eq!(offer(Os::Other, Installed, true), None);
        assert_eq!(offer(Os::Other, Portable, true), None);
        assert_eq!(offer(Os::MacOs, Portable, true), None);
        // The window's numbers.
        assert_eq!(Offer::number(None), 0);
        assert_eq!(Offer::number(Some(Offer::DeleteCompletely)), 1);
        assert_eq!(Offer::number(Some(Offer::OpenUninstaller)), 2);
    }

    #[test]
    fn the_uninstaller_is_the_file_beside_the_executable() {
        assert_eq!(
            uninstaller_beside(Path::new("/p/demo/demo-bin.exe")),
            Path::new("/p/demo").join(UNINSTALLER)
        );
    }

    #[test]
    fn a_program_run_from_a_build_folder_is_offered_nothing() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        // There is no uninstaller beside it, and it is no app bundle.
        let exe = root.path().join("bin").join("app");
        assert_eq!(detect(&paths, Some(&exe)), None);
        assert_eq!(detect(&paths, None), None);
    }

    #[test]
    fn the_backup_goes_to_the_documents_folder_under_its_name() {
        assert_eq!(
            backup_target(Path::new("/home/a/Documents")),
            Path::new("/home/a/Documents").join(backup_file_name())
        );
    }

    #[test]
    fn the_list_names_what_goes_and_the_app_last() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        let bundle = root.path().join("Demo.app");

        let existing: Vec<_> = paths
            .owned_paths()
            .into_iter()
            .filter(|owned| owned.exists())
            .collect();
        let lines = plan_lines(&paths, Some(&bundle)).expect("a list");

        // The folders made above. (On macOS the preferences are a defaults domain the cleanup
        // clears, not a file it lists, and no such file was made.)
        for owned in paths.owned_paths().iter().filter(|owned| owned.exists()) {
            assert!(
                lines
                    .iter()
                    .any(|line| line.contains(&owned.display().to_string())),
                "{owned:?} in {lines:?}"
            );
        }
        assert!(
            lines
                .last()
                .is_some_and(|line| line.contains(&bundle.display().to_string())),
            "{lines:?}"
        );
        // Without a bundle (nothing to move) the list is the cleanup's own.
        let plain = plan_lines(&paths, None).expect("a list");
        assert_eq!(plain, lines[..lines.len() - 1]);
        // Listing deletes nothing.
        for owned in existing {
            assert!(owned.exists(), "{owned:?}");
        }
    }

    #[test]
    fn the_app_goes_to_the_trash_after_its_data_and_only_then() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        let trashed = root.path().join("Trash").join("Demo.app");
        let seen = std::cell::Cell::new(false);

        let outcome = remove(&paths, Box::new(FakeBackend::new()), || {
            // By the time the app is moved, what it owned is gone.
            seen.set(paths.owned_paths().iter().all(|owned| !owned.exists()));
            Ok(trashed.clone())
        });

        assert_eq!(outcome, Outcome::Done { trashed });
        assert!(seen.get(), "the data was deleted before the trash step");
    }

    #[test]
    fn an_app_whose_data_remains_is_not_moved_to_the_trash() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        // A ledger file that is refused stops the cleanup before anything is deleted.
        std::fs::write(
            paths.data_dir().join(rf_ledger::Ledger::FILE_NAME),
            "this is no ledger file and is refused",
        )
        .expect("a ledger file");
        let called = std::cell::Cell::new(false);
        // What exists to be kept (the macOS preferences are a defaults domain, not a file).
        let existing: Vec<_> = paths
            .owned_paths()
            .iter()
            .filter(|owned| owned.exists())
            .collect();

        let outcome = remove(&paths, Box::new(FakeBackend::new()), || {
            called.set(true);
            Ok(PathBuf::from("trash"))
        });

        assert!(!called.get(), "the app stays when its data does");
        let Outcome::Incomplete { lines } = outcome else {
            panic!("expected an incomplete removal");
        };
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_ne!(existing.len(), 0);
        for owned in existing {
            assert!(owned.exists(), "{owned:?} stays");
        }
    }

    #[test]
    fn an_app_that_cannot_be_moved_says_so_after_its_data_is_gone() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");

        let outcome = remove(&paths, Box::new(FakeBackend::new()), || {
            Err(io::Error::other("no permission"))
        });

        let Outcome::NotTrashed { reason, lines } = outcome else {
            panic!("expected a bundle left behind");
        };
        assert!(reason.contains("no permission"), "{reason}");
        assert_eq!(lines.last().map(String::as_str), Some("정리를 마쳤습니다."));
        for owned in paths.owned_paths() {
            assert!(!owned.exists(), "{owned:?} is deleted");
        }
    }

    #[test]
    fn the_settings_are_copied_before_and_a_failed_copy_changes_nothing() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        std::fs::write(
            paths.config_dir().join(settings::FILE_NAME),
            "theme = \"dark\"\n",
        )
        .expect("settings");
        let documents = root.path().join("home").join("Documents");
        std::fs::create_dir_all(&documents).expect("documents");
        let to = backup_target(&documents);
        // A backup of an earlier removal is not written over.
        std::fs::write(&to, "older").expect("older");

        let copy = back_up_settings(&paths, &to)
            .expect("copied")
            .expect("a copy");

        assert_eq!(std::fs::read_to_string(&to).expect("kept"), "older");
        assert_eq!(
            std::fs::read_to_string(&copy).expect("copy"),
            "theme = \"dark\"\n"
        );
        assert_ne!(
            copy.canonicalize().expect("exists"),
            to.canonicalize().expect("exists")
        );
        // A folder that is not there fails, and the settings stay where they were.
        let missing = root.path().join("no such folder").join("b.toml");
        assert!(back_up_settings(&paths, &missing).is_err());
        assert!(paths.config_dir().join(settings::FILE_NAME).is_file());
        // And one inside what is about to be deleted is refused.
        let inside = paths.config_dir().join("b.toml");
        let error = back_up_settings(&paths, &inside).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!inside.exists());
    }

    #[test]
    fn no_settings_means_nothing_to_back_up() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        let to = root.path().join("b.toml");
        assert_eq!(back_up_settings(&paths, &to).expect("no error"), None);
        assert!(!to.exists());
    }
}
