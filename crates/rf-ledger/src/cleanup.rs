//! Clean uninstall (PRD INST-04): put every recorded system change back, then delete everything
//! the app owns.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rf_core::identity::{BUNDLE_ID, TEMP_PREFIX};
use rf_core::{AppPaths, InstallMode};

use crate::backend::SystemBackend;
use crate::error::LedgerError;
use crate::ledger::{EntryId, Ledger, RevertReport};
use crate::store;

/// One step of a clean uninstall, as [`plan_cleanup`] lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CleanupAction {
    /// Put back the before-state of a recorded change.
    Revert {
        id: EntryId,
        /// The change's target, as displayed in logs.
        change: String,
    },
    /// Remove every key of the app's preferences domain (macOS), through the preferences
    /// daemon, which could otherwise write its cached values back to the file.
    ClearDomain(String),
    /// Remove a file, or a folder with everything in it. A link is removed itself; what it
    /// leads to is never touched.
    Delete(PathBuf),
    /// Remove a folder only if it is empty: the portable data folder, which may hold someone
    /// else's files too. A folder that still holds anything is left as it is, and that is no
    /// failure.
    RemoveIfEmpty(PathBuf),
}

/// What [`run_cleanup`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanupReport {
    /// What the revert of the recorded changes settled.
    pub reverted: RevertReport,
    /// The steps after the revert that succeeded, in order.
    pub done: Vec<CleanupAction>,
    /// The steps after the revert that failed, with why. The steps after a failed one still ran.
    pub failed: Vec<(CleanupAction, String)>,
    /// Whether nothing was cleared or deleted because a change is still recorded: the ledger
    /// file and the data folder stay, so that a next run can revert what is left.
    pub deletes_skipped: bool,
}

impl CleanupReport {
    /// Whether the clean uninstall is finished: every change settled and every step done.
    pub fn is_complete(&self) -> bool {
        self.reverted.is_clean() && self.failed.is_empty() && !self.deletes_skipped
    }
}

/// Lists what [`run_cleanup`] would do, in this order, without changing anything: it reads the
/// ledger file and folder listings only, and calls no backend.
///
/// 1. `Revert` for every recorded change, newest first (`Pending` ones too).
/// 2. `ClearDomain` for the app's preferences domain if the layout has a preferences file
///    (macOS), even when the file is gone: the preferences daemon may still hold the values.
/// 3. `Delete` for each of [`AppPaths::owned_paths`] that is there, link or not, in that order.
///    A path that cannot be looked at is listed too, so that deleting it reports why. In
///    portable mode the one owned path is a plain `data` folder next to the executable, which
///    may hold someone else's files, so it is not deleted whole: instead `Delete` for each of
///    the config, data, cache, log and temp folders the app makes in it that is there, then
///    `RemoveIfEmpty` for the folder itself. If that folder is a link, it is deleted like any
///    owned link, which removes only the link.
/// 4. `Delete` for every entry of the shared temp folder whose name starts with
///    `identity::TEMP_PREFIX`, by name; none if the folder is missing. A temp folder inside an
///    owned path (portable mode) is not listed: it is one of the app's folders above and goes
///    whole.
///
/// A ledger file that [`Ledger::open`] refuses (it does not parse, or a newer build wrote it)
/// is refused here with the same error. A temp folder that cannot be listed is an error too.
///
/// In portable mode the ledger file is not read and no preferences domain is cleared: the list
/// is the app's folders in the data folder and the data folder alone.
pub fn plan_cleanup(paths: &AppPaths) -> Result<Vec<CleanupAction>, LedgerError> {
    let mut plan: Vec<CleanupAction> = Ledger::inspect(paths)?
        .into_iter()
        .rev()
        .map(|entry| CleanupAction::Revert {
            id: entry.id,
            change: entry.change.target.to_string(),
        })
        .collect();
    if paths.mode() != InstallMode::Portable && paths.preferences_file().is_some() {
        plan.push(CleanupAction::ClearDomain(BUNDLE_ID.to_owned()));
    }
    let owned = paths.owned_paths();
    if paths.mode() == InstallMode::Portable {
        for root in owned {
            plan.extend(portable_steps(paths, root));
        }
    } else {
        plan.extend(
            owned
                .iter()
                .filter(|path| is_there(path))
                .cloned()
                .map(CleanupAction::Delete),
        );
    }
    let temp = paths.temp_dir();
    if !owned.iter().any(|path| temp.starts_with(path)) {
        plan.extend(temp_entries(temp)?.into_iter().map(CleanupAction::Delete));
    }
    Ok(plan)
}

/// Runs a clean uninstall: reverts every recorded system change, newest first, then runs the
/// other steps of [`plan_cleanup`] in order (PRD INST-04).
///
/// The order is the point. While a change is recorded, the ledger file is the only record that
/// something is still owed, so nothing is cleared or deleted unless the revert settled every
/// change: if one fails, the report says `deletes_skipped`, and the ledger file and the data
/// folder stay. The revert saves the ledger file after each change it settles, so a process
/// that dies part-way leaves every change it did not settle recorded; running this again
/// settles only those, then deletes. The ledger file itself is deleted only with the data
/// folder, once it records nothing.
///
/// A ledger file that [`Ledger::open`] refuses stops everything with that error: nothing is
/// reverted or deleted. A step after the revert that fails is reported in `failed`, and the
/// steps after it still run; running this again retries what is left.
///
/// The caller (the uninstaller, the macOS "delete completely" command) first ends the app's
/// other processes (INST-04 (1)) and closes its own log file: Windows cannot delete an open
/// file.
///
/// In portable mode no backend method is called and the ledger file is not read: only the
/// app's folders in the data folder are deleted, and the data folder itself if that leaves it
/// empty.
pub fn run_cleanup(
    paths: &AppPaths,
    backend: Box<dyn SystemBackend>,
) -> Result<CleanupReport, LedgerError> {
    let (mut ledger, _settled) = Ledger::open(paths, backend)?;
    let mut report = CleanupReport {
        reverted: ledger.revert_all(),
        ..CleanupReport::default()
    };
    if !report.reverted.is_clean() {
        report.deletes_skipped = true;
        return Ok(report);
    }
    let plan = plan_cleanup(paths)?;
    // The plan reads the ledger file again. After a clean revert it records nothing; should it
    // record a change after all, deleting the file would forget what is owed.
    if plan
        .iter()
        .any(|action| matches!(action, CleanupAction::Revert { .. }))
    {
        report.deletes_skipped = true;
        return Ok(report);
    }
    for action in plan {
        let result = match &action {
            // There is none, as checked above.
            CleanupAction::Revert { .. } => continue,
            CleanupAction::ClearDomain(domain) => ledger
                .backend_mut()
                .clear_domain(domain)
                .map_err(|e| e.to_string()),
            CleanupAction::Delete(path) => delete(path).map_err(|e| e.to_string()),
            CleanupAction::RemoveIfEmpty(path) => remove_if_empty(path).map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => report.done.push(action),
            Err(error) => report.failed.push((action, error)),
        }
    }
    Ok(report)
}

/// Whether something is at `path`, a link included. Only a path that is known to be missing is
/// not there.
fn is_there(path: &Path) -> bool {
    !matches!(fs::symlink_metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

/// The portable steps for `root`, the one owned path in portable mode: `Delete` for each folder
/// of the layout that is there, then `RemoveIfEmpty` for `root`. A `root` that is not a real
/// folder (a link to another drive) is deleted as any owned link is, which removes only the
/// link: the app's folders behind it are not reached through it.
fn portable_steps(paths: &AppPaths, root: &Path) -> Vec<CleanupAction> {
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Ok(meta) if !meta.file_type().is_dir() => vec![CleanupAction::Delete(root.to_path_buf())],
        // A real folder, or one that cannot be looked at: the steps report why they fail.
        _ => {
            let layout = [
                paths.config_dir(),
                paths.data_dir(),
                paths.cache_dir(),
                paths.log_dir(),
                paths.temp_dir(),
            ];
            let mut steps: Vec<CleanupAction> = layout
                .into_iter()
                .filter(|dir| is_there(dir))
                .map(|dir| CleanupAction::Delete(dir.to_path_buf()))
                .collect();
            steps.push(CleanupAction::RemoveIfEmpty(root.to_path_buf()));
            steps
        }
    }
}

/// The entries of the shared temp folder `temp` whose names start with `TEMP_PREFIX`, by name;
/// none if the folder is missing.
fn temp_entries(temp: &Path) -> Result<Vec<PathBuf>, LedgerError> {
    let fail = |e| store::io_error("read_dir", temp, e);
    let listing = match fs::read_dir(temp) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(fail(e)),
    };
    let mut found = Vec::new();
    for entry in listing {
        let entry = entry.map_err(fail)?;
        if entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(TEMP_PREFIX.as_bytes())
        {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// Removes `path` without following a link (PRD principle 1), judged by what is there now: a
/// real folder with everything in it (`remove_dir_all` removes the links inside without
/// following them), a Windows folder link or junction with `remove_dir`, and anything else with
/// `remove_file`, a Unix link to a folder included. A path that is gone already counts as
/// removed.
fn delete(path: &Path) -> io::Result<()> {
    let removed = fs::symlink_metadata(path).and_then(|meta| {
        let kind = meta.file_type();
        #[cfg(windows)]
        if std::os::windows::fs::FileTypeExt::is_symlink_dir(&kind) {
            return fs::remove_dir(path);
        }
        if kind.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    });
    match removed {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Removes the folder `path` if it is empty. A folder that still holds anything is left as it
/// is, and so is a path that is gone; neither is a failure. It is planned for a real folder
/// only, and `remove_dir` never touches what a link leads to.
fn remove_if_empty(path: &Path) -> io::Result<()> {
    match fs::remove_dir(path) {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        other => other,
    }
}
