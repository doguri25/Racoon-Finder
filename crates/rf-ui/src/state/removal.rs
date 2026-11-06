//! Removing the app from the window's menu (PRD INST-05): 보기 ▸ 설정 ▸ "Racoon Finder 완전
//! 삭제…" on macOS and "제거 프로그램 열기" for an installed app on Windows ([`Offer`], chosen
//! once at the start). A child of [`state`](super), so these methods see the fields of
//! [`AppState`].
//!
//! 완전 삭제… goes in steps, each a question or a worker's answer:
//!
//! 1. The menu item ([`AppState::removal_asked`]) has a worker list what would go
//!    ([`Effect::Removal(RemovalWork::Plan)`]); the window does not look at the disk.
//! 2. The list comes ([`AppState::removal_planned`]): a confirmation shows it, with a checkbox
//!    for 설정 백업 ([`Confirm::remove_app`]).
//! 3. Confirmed, with jobs running, a second question says they stop for good; 취소 there is
//!    the wait for them to end. Without jobs, or once that is answered, the removal begins
//!    ([`RemovalWork::Run`]): the window is the closing note, as when the app ends, and takes
//!    no question and no click until the worker answers.
//! 4. A backup that failed leaves everything as it was ([`RemovalStop::BackupFailed`]); a
//!    removal that stopped part-way says how far it got in a window, and the app stays
//!    ([`RemovalStop::Partway`]); a complete one ends the app, from the worker.
//!
//! No `AppState` borrow is held while a worker runs: the effects are carried out after the
//! change, and the worker's answers come back through the controller as any other news.

use super::{AppState, Effect};
use crate::dialogs::{Confirm, ConfirmPurpose, Failures, Modal};
use crate::removal::Offer;
use crate::words;

/// What the window asks of the removal's workers ([`Effect::Removal`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalWork {
    /// List on a worker thread what 완전 삭제… would delete
    /// ([`removal::plan_lines`](crate::removal::plan_lines)), then hand the list to
    /// [`AppState::removal_planned`].
    Plan,
    /// Start the uninstaller that sits beside the executable (제거 프로그램 열기), on a worker
    /// thread; a failure shows as a notice.
    OpenUninstaller,
    /// 완전 삭제… was confirmed ([`AppState::begin_removal`] has closed the windows): back the
    /// settings up first if `backup`, then end the app's engine, delete what it owns, move the
    /// app to the trash and end the app, all on worker threads. A stop comes back to
    /// [`AppState::removal_stopped`].
    Run { backup: bool },
}

/// Why 완전 삭제… stopped before it was complete (the worker's news to
/// [`AppState::removal_stopped`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalStop {
    /// The settings could not be copied, so nothing was changed: the app goes on as it was.
    /// The text says so.
    BackupFailed(String),
    /// It stopped part-way: the app's engine has ended and some of its data may be gone. The
    /// lines say how far it got.
    Partway(Vec<String>),
}

impl AppState {
    /// Takes what the bottom of 보기 ▸ 설정 offers for removing the app ([`Offer`]).
    pub fn set_removal_offer(&mut self, offer: Option<Offer>) {
        self.removal_offer = offer;
    }

    /// What the bottom of 보기 ▸ 설정 offers for removing the app.
    pub fn removal_offer(&self) -> Option<Offer> {
        self.removal_offer
    }

    /// Whether 완전 삭제… works now: the window shows the note of it.
    pub fn removing(&self) -> bool {
        self.removing
    }

    /// The menu item at the bottom of 설정 was chosen: a worker lists what 완전 삭제… would
    /// delete, or the uninstaller starts. Nothing while one list is being made, while the
    /// removal works, and as the app ends, or when the menu offers nothing.
    pub fn removal_asked(&mut self) -> Vec<Effect> {
        if self.quitting || self.removing || self.planning_removal {
            return Vec::new();
        }
        match self.removal_offer {
            Some(Offer::DeleteCompletely) => {
                self.planning_removal = true;
                vec![Effect::Removal(RemovalWork::Plan)]
            }
            Some(Offer::OpenUninstaller) => vec![Effect::Removal(RemovalWork::OpenUninstaller)],
            None => Vec::new(),
        }
    }

    /// The worker listed what 완전 삭제… would delete (`Err`: why it could not, in words): the
    /// confirmation shows the list.
    pub fn removal_planned(&mut self, plan: Result<Vec<String>, String>) -> Vec<Effect> {
        self.planning_removal = false;
        if self.quitting {
            return Vec::new();
        }
        match plan {
            Ok(lines) => {
                self.open(Modal::Confirm(Confirm::remove_app(lines)));
                self.follow_background()
            }
            Err(reason) => vec![Effect::Notice(words::removal_plan_failed(&reason))],
        }
    }

    /// The checkbox of the confirmation that shows was flipped.
    pub fn confirm_check_toggled(&mut self) {
        if let Some(Modal::Confirm(confirm)) = &mut self.modal
            && !confirm.check.is_empty()
        {
            confirm.checked = !confirm.checked;
        }
    }

    /// 완전 삭제… was confirmed, with the settings backed up first if `backup`. While jobs run
    /// they are asked about first; without, the removal begins.
    pub(super) fn removal_confirmed(&mut self, backup: bool) -> Vec<Effect> {
        let running = self.jobs.active();
        if running == 0 {
            return self.begin_removal(backup);
        }
        self.modal = Some(Modal::Confirm(Confirm::remove_with_jobs(running, backup)));
        Vec::new()
    }

    /// The removal begins: the window is the closing note and takes no click or key. Windows
    /// and questions that were open are kept aside, not answered or dropped: the backup may
    /// still stop the removal ([`AppState::removal_stopped`] puts them back). The worker does
    /// the rest ([`RemovalWork::Run`]).
    pub(super) fn begin_removal(&mut self, backup: bool) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        let asking_removal = |modal: &Modal| {
            matches!(
                modal,
                Modal::Confirm(confirm)
                    if matches!(
                        confirm.purpose,
                        ConfirmPurpose::RemoveApp | ConfirmPurpose::RemoveAppWithJobs { .. }
                    )
            )
        };
        self.removal_stash = self
            .modal
            .take()
            .into_iter()
            .chain(self.later.drain(..))
            .filter(|modal| !asking_removal(modal))
            .collect();
        self.toast = None;
        self.quitting = true;
        self.removing = true;
        vec![Effect::Removal(RemovalWork::Run { backup })]
    }

    /// The removal stopped before it was complete, as the worker tells. Before anything was
    /// touched ([`RemovalStop::BackupFailed`]) the windows and the job questions that were open
    /// come back as they were; after the engine ended ([`RemovalStop::Partway`]) they are gone
    /// with it and a window says how far the removal got.
    pub fn removal_stopped(&mut self, stop: RemovalStop) -> Vec<Effect> {
        if !self.removing {
            return Vec::new();
        }
        self.removing = false;
        self.quitting = false;
        let stash = std::mem::take(&mut self.removal_stash);
        match stop {
            RemovalStop::BackupFailed(text) => {
                for modal in stash {
                    self.open(modal);
                }
                vec![Effect::Notice(text)]
            }
            RemovalStop::Partway(mut lines) => {
                lines.insert(0, words::removal_stopped_note().to_owned());
                self.open(Modal::Failures(Failures {
                    title: words::removal_stopped_title().to_owned(),
                    lines,
                }));
                Vec::new()
            }
        }
    }

    /// Whether the confirmation that shows is for 완전 삭제… (either question).
    pub fn removal_asked_about(&self) -> bool {
        matches!(
            &self.modal,
            Some(Modal::Confirm(confirm))
                if matches!(
                    confirm.purpose,
                    ConfirmPurpose::RemoveApp | ConfirmPurpose::RemoveAppWithJobs { .. }
                )
        )
    }
}
