//! How the [`Controller`] carries out the removal of the app (INST-05): the list of what
//! 완전 삭제… deletes, the uninstaller of an installed app on Windows, and the removal itself.
//!
//! The removal runs in two parts on worker threads, with the UI thread between them (the
//! engine, the saver and the watches are the controller's to stop):
//!
//! 1. A worker copies the settings to the documents folder when asked to, after writing the
//!    settings the window shows now so that the copy is current. A copy that fails stops the
//!    removal there: nothing was changed, and the window is the window again.
//! 2. The controller stops the thumbnails, the look for requests and the saver (the settings
//!    are never written again), and a second worker ends the job engine, then runs
//!    [`removal::remove`]: the recorded changes of the system are put back, what the app owns
//!    is deleted, and only then the app goes to the trash. The app ends when that is complete;
//!    when it is not, a window says how far it got and the app stays.
//!
//! Nothing here borrows the `AppState` across a wait: every answer comes back by [`post`].

use std::path::{Path, PathBuf};

use rf_core::AppPaths;
use rf_platform::KnownFolder;

use super::{Controller, FINAL_SAVE_WAIT, end_work};
use crate::removal::{self, Offer, Outcome};
use crate::state::{RemovalStop, RemovalWork};
use crate::ui_bridge::post;
use crate::words;

/// Where the app runs from and what its menu offers for removing it.
#[derive(Clone, Debug)]
pub(super) struct Context {
    /// The folders of the app.
    pub paths: AppPaths,
    /// The executable that runs, when the system says where it is.
    pub exe: Option<PathBuf>,
    /// What the bottom of 보기 ▸ 설정 offers.
    pub offer: Option<Offer>,
}

impl Controller {
    /// Carries out a step of removing the app ([`Effect::Removal`](crate::state::Effect)).
    pub(super) fn removal_work(&self, work: RemovalWork) {
        match work {
            RemovalWork::Plan => self.plan_removal(),
            RemovalWork::OpenUninstaller => self.open_uninstaller(),
            RemovalWork::Run { backup } => self.remove_app(backup),
        }
    }

    /// Has a worker list what 완전 삭제… would delete; the list comes to
    /// [`AppState::removal_planned`](crate::state::AppState::removal_planned).
    pub(super) fn plan_removal(&self) {
        let Some(context) = self.removal.borrow().clone() else {
            return self
                .update(|state| state.removal_planned(Err(words::background_failed().to_owned())));
        };
        let bundle = context.exe.as_deref().and_then(rf_platform::app_bundle_of);
        let started = std::thread::Builder::new()
            .name("rf-ui-plan-removal".to_owned())
            .spawn(move || {
                let plan = removal::plan_lines(&context.paths, bundle.as_deref())
                    .map_err(|error| error.to_string());
                post(move |c| c.update(|state| state.removal_planned(plan)));
            });
        if started.is_err() {
            self.update(|state| state.removal_planned(Err(words::background_failed().to_owned())));
        }
    }

    /// Starts the uninstaller beside the executable (제거 프로그램 열기). It runs the same
    /// cleanup as `--cleanup`, which asks this app to end.
    pub(super) fn open_uninstaller(&self) {
        let exe = self
            .removal
            .borrow()
            .as_ref()
            .and_then(|context| context.exe.clone());
        let Some(exe) = exe else {
            return self.notice(&words::uninstaller_failed(words::background_failed()));
        };
        let uninstaller = removal::uninstaller_beside(&exe);
        let started = std::thread::Builder::new()
            .name("rf-ui-uninstaller".to_owned())
            .spawn(move || {
                let started = std::process::Command::new(&uninstaller)
                    .stdin(std::process::Stdio::null())
                    .spawn();
                if let Err(error) = started {
                    post(move |c| c.notice(&words::uninstaller_failed(&error.to_string())));
                }
            });
        if started.is_err() {
            self.notice(words::background_failed());
        }
    }

    /// 완전 삭제… was confirmed ([`AppState::begin_removal`](crate::state::AppState)): the
    /// first part, the backup, on a worker.
    pub(super) fn remove_app(&self, backup: bool) {
        let Some(context) = self.removal.borrow().clone() else {
            return self.removal_stopped(RemovalStop::BackupFailed(
                words::background_failed().to_owned(),
            ));
        };
        // What the window shows now, so that the copy of the settings is current. Nothing is
        // closed yet: the removal may still stop.
        let text = if backup { self.settings_text() } else { None };
        let saver = self.saver.clone();
        let started = std::thread::Builder::new()
            .name("rf-ui-remove".to_owned())
            .spawn(move || {
                let documents = documents_folder();
                let saved = || text.and_then(|text| saver.save_within(text, FINAL_SAVE_WAIT));
                match back_up_first(&context.paths, backup, documents.as_deref(), saved) {
                    Ok(copy) => post(move |c| c.removal_continue(copy)),
                    Err(text) => post(move |c| c.removal_stopped(RemovalStop::BackupFailed(text))),
                }
            });
        if started.is_err() {
            self.removal_stopped(RemovalStop::BackupFailed(
                words::background_failed().to_owned(),
            ));
        }
    }

    /// The backup is done (`copy`: where it went, if there were settings): the second part.
    fn removal_continue(&self, copy: Option<PathBuf>) {
        let Some(context) = self.removal.borrow().clone() else {
            return;
        };
        // From here on nothing writes into the folders being deleted: no thumbnail, no look at
        // requests, no save of the settings, and no job.
        self.thumber.stop();
        let mut end = self.end_parts();
        end.last = None;
        let saver = self.saver.clone();
        let backed_up = copy.map(|copy| words::removal_backup_done(&copy));
        let started = std::thread::Builder::new()
            .name("rf-ui-remove-app".to_owned())
            .spawn(move || {
                end_work(end);
                saver.stop_within(FINAL_SAVE_WAIT);
                let outcome = removal::remove(
                    &context.paths,
                    rf_ledger::os_backend(),
                    rf_platform::move_app_bundle_to_trash,
                );
                match outcome {
                    Outcome::Done { .. } => {
                        let _ = slint::invoke_from_event_loop(|| {
                            let _ = slint::quit_event_loop();
                        });
                    }
                    Outcome::Incomplete { lines } => {
                        stopped(backed_up, lines);
                    }
                    Outcome::NotTrashed { reason, mut lines } => {
                        lines.insert(0, words::removal_not_trashed(&reason));
                        stopped(backed_up, lines);
                    }
                }
            });
        if started.is_err() {
            self.removal_stopped(RemovalStop::Partway(vec![
                words::background_failed().to_owned(),
            ]));
        }
    }

    /// The removal stopped: the window says so.
    fn removal_stopped(&self, stop: RemovalStop) {
        self.update(|state| state.removal_stopped(stop));
    }
}

/// Tells the window how far the removal got, `lines` in all, after the backup `backed_up` if
/// one was made.
fn stopped(backed_up: Option<String>, mut lines: Vec<String>) {
    if let Some(done) = backed_up {
        lines.insert(0, done);
    }
    post(move |c| c.removal_stopped(RemovalStop::Partway(lines)));
}

/// The documents folder (the home folder where the system has none), as a local path.
fn documents_folder() -> Option<PathBuf> {
    rf_platform::known_folder(KnownFolder::Documents)
        .or_else(|| rf_platform::known_folder(KnownFolder::Home))
        .and_then(|folder| folder.as_local().map(Path::to_path_buf))
}

/// The first part of the removal: the settings copied into the folder `documents` when `backup`
/// asks for it, after `save` has written the settings the window shows. `Err` is the sentence
/// that says nothing was deleted.
fn back_up_first(
    paths: &AppPaths,
    backup: bool,
    documents: Option<&Path>,
    save: impl FnOnce() -> Option<std::io::Result<()>>,
) -> Result<Option<PathBuf>, String> {
    if !backup {
        return Ok(None);
    }
    // Written or not (a disk that hangs, settings that may not be written), the copy is of the
    // file there is.
    let _ = save();
    let Some(documents) = documents else {
        return Err(words::cleanup_backup_failed(
            Path::new(removal::BACKUP_FILE_NAME),
            "the documents folder is not known",
        ));
    };
    let to = removal::backup_target(documents);
    removal::back_up_settings(paths, &to)
        .map_err(|error| words::cleanup_backup_failed(&to, &error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use rf_core::{BaseDirs, Os};

    use super::*;
    use crate::settings;

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
    fn without_the_checkbox_nothing_is_copied_or_saved() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        let saved = Cell::new(false);
        let copy = back_up_first(&paths, false, Some(root.path()), || {
            saved.set(true);
            None
        });
        assert_eq!(copy, Ok(None));
        assert!(!saved.get());
        assert_eq!(std::fs::read_dir(root.path()).expect("listed").count(), 1);
    }

    #[test]
    fn the_settings_shown_are_saved_and_then_copied_to_the_documents_folder() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        let file = paths.config_dir().join(settings::FILE_NAME);
        std::fs::write(&file, "theme = \"light\"\n").expect("settings");
        let documents = root.path().join("home").join("Documents");
        std::fs::create_dir_all(&documents).expect("documents");

        // The save is what makes the file current: the copy is made after it.
        let copy = back_up_first(&paths, true, Some(&documents), || {
            std::fs::write(&file, "theme = \"dark\"\n").expect("saved");
            Some(Ok(()))
        })
        .expect("copied")
        .expect("a copy");

        assert_eq!(
            copy.canonicalize().expect("exists"),
            documents
                .join(removal::BACKUP_FILE_NAME)
                .canonicalize()
                .expect("exists")
        );
        assert_eq!(
            std::fs::read_to_string(&copy).expect("copy"),
            "theme = \"dark\"\n"
        );
        // The settings themselves are still where they were: nothing is deleted here.
        assert!(file.is_file());
    }

    #[test]
    fn a_backup_that_cannot_be_made_says_nothing_was_deleted() {
        let root = tempfile::tempdir().expect("a temp dir");
        let paths = installed_under(root.path());
        paths.ensure_dirs().expect("folders");
        std::fs::write(paths.config_dir().join(settings::FILE_NAME), "x").expect("settings");

        // No documents folder.
        let none = back_up_first(&paths, true, None, || None).expect_err("no folder");
        assert!(none.contains("아무것도 지우지 않았습니다"), "{none}");
        // A documents folder that is not there.
        let missing = root.path().join("no such folder");
        let error = back_up_first(&paths, true, Some(&missing), || None).expect_err("no folder");
        assert!(error.contains("아무것도 지우지 않았습니다"), "{error}");
        // One inside what the removal deletes.
        let inside = paths.config_dir();
        let error = back_up_first(&paths, true, Some(inside), || None).expect_err("refused");
        assert!(error.contains("아무것도 지우지 않았습니다"), "{error}");
        assert!(paths.config_dir().join(settings::FILE_NAME).is_file());
        assert!(!paths.config_dir().join(removal::BACKUP_FILE_NAME).exists());
    }
}
