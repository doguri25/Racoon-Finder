//! The main window's state, and what its keys, clicks and the loader's events do to it.
//!
//! [`AppState`] is plain data on the UI thread: the two panes, which one has the keyboard,
//! whether hidden items show, the folder tree of the sidebar, the jobs of the job panel, and
//! the window that shows over the main window (a [`Modal`]). Its methods change it and return
//! the [`Effect`]s that need a worker thread, the job engine or the screen, which the
//! [`Controller`](crate::controller::Controller) carries out: a listing to start, a link to
//! follow or a vanished folder's nearest parent to find, a file to open, a job to submit or a
//! question to answer, a notice, a row to scroll to. Keeping the two apart lets the tests
//! drive the state without a window.
//!
//! Every change of a file is a job ([`Effect::Job`]); nothing here or in the controller writes
//! a file itself.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use rf_core::{EntryKind, ErrorKind, VPath};
use rf_jobs::{
    DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobEvent, JobSpec, Outcome,
    TransferMode, TransferOptions, TransferSpec, UndoRecord,
};
use rf_vfs::{SortKey, VisibilityFilter};

use crate::dialogs::{
    Confirm, ConfirmPurpose, ConflictChoice, ConflictQuestion, DeleteWhy, Failures, Modal,
    NameCheck, NameDialog, PreflightQuestion, Recovery,
};
use crate::jobs::{Ended, History, JobEffect, JobRow, JobRows, Started};
use crate::keys::{Command, Mods};
use crate::loader::{Followed, LoadEvent, LoadRequest};
use crate::pane::{Handled, OpenAction, Pane, Tab, ancestors};
use crate::places::Place;
use crate::resolver::{Asked, Question};
use crate::tree::Tree;
use crate::words::{self, JobWork};

/// The left pane's position in [`AppState::panes`].
pub const LEFT: usize = 0;
/// The right pane's position in [`AppState::panes`].
pub const RIGHT: usize = 1;

/// The shortest time between two automatic reloads of a folder whose watch ended while the
/// folder is still there, so that a watch that keeps failing cannot reload it in a loop.
pub const AUTO_RELOAD_GAP: Duration = Duration::from_secs(5);

/// What a change of the [`AppState`] needs done outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// List and watch a tab's folder ([`Loader::request`](crate::loader::Loader::request)).
    Load(LoadRequest),
    /// The watch of the tab with `generation` ended: find on a worker thread where to go
    /// ([`Loader::nearest_existing_parent`](crate::loader::Loader::nearest_existing_parent) of
    /// `path`, after [`VANISHED_SETTLE`](crate::controller::VANISHED_SETTLE)), then call
    /// [`AppState::found_existing`].
    FindExisting { generation: u64, path: VPath },
    /// The row opened in the tab with `generation` is a link: follow it on a worker thread
    /// ([`Loader::follow`](crate::loader::Loader::follow)), then hand the result, which
    /// carries `generation`, to [`AppState::followed`].
    Follow { generation: u64, link: VPath },
    /// Open the file with the system's default app.
    OpenFile(VPath),
    /// The folder of the tab with `generation` came back after its watch ended, within
    /// [`AUTO_RELOAD_GAP`] of its last automatic reload: call [`AppState::reload_due`] after
    /// `after` (a one-shot timer on the UI thread).
    ReloadLater {
        generation: u64,
        path: VPath,
        after: Duration,
    },
    /// Show a notice in the corner of the window.
    Notice(String),
    /// Show the path of pane `side` as text to edit.
    EditPath(usize),
    /// Scroll pane `side` so that `row` shows.
    ScrollTo { side: usize, row: usize },
    /// List the folders in a folder of the tree
    /// ([`Loader::list_folders`](crate::loader::Loader::list_folders)); the listing comes
    /// back to [`AppState::loaded`].
    LoadChildren(LoadRequest),
    /// Scroll the folder tree so that row `row`, the active pane's folder just found there,
    /// shows.
    ScrollTree(usize),
    /// Ask the job engine for something ([`Jobs::carry_out`](crate::jobs::Jobs::carry_out)).
    /// A job it starts goes to [`AppState::track`], and what the history can do next then to
    /// [`AppState::set_history`].
    Job(JobEffect),
    /// Find on a worker thread whether the location of `folder` has a trash
    /// (`rf_platform::trash_supported`), then call [`AppState::trash_checked`] with `paths`.
    CheckTrash { folder: VPath, paths: Vec<VPath> },
    /// Show a toast with the message `text` and the button `action`, whose click comes back
    /// to [`AppState::toast_clicked`] with `token`.
    Toast {
        token: u64,
        text: String,
        action: String,
    },
    /// End the app ([`crate::jobs::close_down`] on a worker thread, then the end of the event
    /// loop). [`AppState::begin_quit`] has closed the windows and stopped taking questions.
    Quit,
}

/// What the button of the toast on screen does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToastAction {
    /// Undo the job that left `record`, if it is still the next one to undo.
    Undo(UndoRecord),
    /// Show the items the job could not process.
    Details(Failures),
}

/// The window's state on the UI thread: two panes, the one with the keyboard, whether hidden
/// items show, and the folder tree that follows the active pane.
#[derive(Debug)]
pub struct AppState {
    panes: [Pane; 2],
    active: usize,
    show_hidden: bool,
    /// The sidebar's folder tree, whose roots are the places.
    tree: Tree,
    /// Where a tab goes when its folder and every parent of it are gone.
    home: Option<VPath>,
    /// When each folder was last reloaded because its watch ended ([`AUTO_RELOAD_GAP`]).
    auto_reloads: HashMap<VPath, Instant>,
    /// The tabs, by generation, with a reload waiting for its gap ([`Effect::ReloadLater`]):
    /// one at most each.
    pending_reloads: HashSet<u64>,
    /// The jobs of the job panel.
    jobs: JobRows,
    /// What the engine's history can undo and redo next, as the engine last told.
    history: History,
    /// The window over the main window, if one shows.
    modal: Option<Modal>,
    /// The windows waiting for the one that shows to close, in the order they came.
    later: VecDeque<Modal>,
    /// The toast on screen whose button does something, by its token.
    toast: Option<(u64, ToastAction)>,
    /// The token of the last toast.
    toast_token: u64,
    /// The app is ending: no window opens and no question shows any more.
    quitting: bool,
    /// Whether the notice of a journal error has shown (once is enough).
    journal_error_told: bool,
}

impl AppState {
    /// The left pane at `left`, the right one at `right`, the left one active, and the
    /// requests for their listings.
    pub fn new(left: VPath, right: VPath, home: Option<VPath>) -> (AppState, Vec<Effect>) {
        let filter = VisibilityFilter::default();
        let (left_pane, left_request) = Pane::new(left, filter);
        let (right_pane, right_request) = Pane::new(right, filter);
        let state = AppState {
            panes: [left_pane, right_pane],
            active: LEFT,
            show_hidden: filter.show_hidden,
            tree: Tree::new(filter.show_hidden),
            home,
            auto_reloads: HashMap::new(),
            pending_reloads: HashSet::new(),
            jobs: JobRows::default(),
            history: History::default(),
            modal: None,
            later: VecDeque::new(),
            toast: None,
            toast_token: 0,
            quitting: false,
            journal_error_told: false,
        };
        (
            state,
            vec![Effect::Load(left_request), Effect::Load(right_request)],
        )
    }

    pub fn panes(&self) -> &[Pane; 2] {
        &self.panes
    }

    /// The pane with the keyboard: [`LEFT`] or [`RIGHT`].
    pub fn active(&self) -> usize {
        self.active
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// The sidebar's folder tree.
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The generation of every tab's last request and of every listing the tree waits for:
    /// the loads still worth running.
    pub fn generations(&self) -> HashSet<u64> {
        self.panes
            .iter()
            .flat_map(Pane::tabs)
            .map(Tab::generation)
            .chain(self.tree.generations())
            .collect()
    }

    /// Gives pane `side` the keyboard.
    pub fn activate(&mut self, side: usize) {
        self.active = side.min(RIGHT);
    }

    /// Carries out the shortcut `command`, held with `mods`, on the active pane. `page` is how
    /// many rows `PageUp` and `PageDown` move. `None` for a command this window does not take
    /// (yet): the key goes on to the system.
    pub fn key(
        &mut self,
        command: Command,
        mods: Mods,
        page: usize,
        now: Instant,
    ) -> Option<Vec<Effect>> {
        // A window over the main window takes the keys itself; one that comes here anyway (or
        // one pressed while the app ends) does nothing.
        if self.modal.is_some() || self.quitting {
            return Some(Vec::new());
        }
        let side = self.active;
        let page = isize::try_from(page.max(1)).unwrap_or(isize::MAX);
        let effects = match command {
            Command::SwitchPane => {
                self.active = RIGHT - side;
                Vec::new()
            }
            Command::Open => match self.tab(side).selection().cursor() {
                Some(row) => self.open_row(side, row),
                None => Vec::new(),
            },
            Command::Up => self.go(side, Tab::go_up),
            Command::Back => self.go(side, Tab::go_back),
            Command::Forward => self.go(side, Tab::go_forward),
            Command::CursorUp => self.move_cursor(side, -1, mods.shift),
            Command::CursorDown => self.move_cursor(side, 1, mods.shift),
            Command::CursorHome => self.move_cursor(side, isize::MIN, mods.shift),
            Command::CursorEnd => self.move_cursor(side, isize::MAX, mods.shift),
            Command::PageUp => self.move_cursor(side, -page, mods.shift),
            Command::PageDown => self.move_cursor(side, page, mods.shift),
            Command::ToggleSelect => {
                let (selection, rows) = self.tab_mut(side).selection_mut();
                if let Some(cursor) = selection.cursor() {
                    selection.toggle(cursor, rows);
                }
                Vec::new()
            }
            Command::SelectAll => {
                let (selection, rows) = self.tab_mut(side).selection_mut();
                selection.select_all(rows);
                Vec::new()
            }
            Command::ClearSelection => {
                self.tab_mut(side).selection_mut().0.clear();
                Vec::new()
            }
            Command::EditPath => vec![Effect::EditPath(side)],
            Command::NewTab => self.add_tab(side),
            Command::CloseTab => {
                let tab = self.panes[side].active();
                self.close_tab(side, tab)
            }
            Command::ToggleHidden => self.toggle_hidden(),
            Command::TypeToSelect(text) => self
                .tab_mut(side)
                .type_to_select(&text, now)
                .map(|row| Effect::ScrollTo { side, row })
                .into_iter()
                .collect(),
            Command::NewFolder => self.new_folder(side),
            Command::Rename => self.rename(side),
            Command::Trash => self.trash(side),
            Command::DeletePermanently => self.delete_permanently(side),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            // Copy and move pick their destination first (PICK), which this window does not
            // have yet; their keys are left to the system.
            Command::CopyTo | Command::MoveTo => return None,
        };
        Some(effects)
    }

    /// A right click on row `row` of pane `side`, before its menu opens: the pane gets the
    /// keyboard, and the row the cursor. A row that is not selected becomes the selection, as
    /// a click would make it; a selected one leaves the selection as it is, so that the menu
    /// acts on all of it.
    pub fn row_menu(&mut self, side: usize, row: usize) -> Vec<Effect> {
        self.activate(side);
        let (selection, rows) = self.tab_mut(side).selection_mut();
        match rows.get(row) {
            Some(entry) if selection.is_selected(&entry.name) => selection.set_cursor(row, rows),
            Some(_) => selection.click(row, rows),
            None => {}
        }
        Vec::new()
    }

    /// Copies or moves the items of pane `side` (the selected rows, or the cursor's row) into
    /// the folder `dest`, with `options`: the job the destination picker (PICK) submits once a
    /// target is chosen. Nothing for a pane with no item to act on.
    pub fn transfer(
        &mut self,
        side: usize,
        mode: TransferMode,
        dest: &VPath,
        options: TransferOptions,
    ) -> Vec<Effect> {
        let sources = self.targets(side);
        if sources.is_empty() {
            return Vec::new();
        }
        match TransferSpec::into_dir(mode, &sources, dest, options) {
            Ok(spec) => vec![submit(JobSpec::Transfer(spec), Some(side))],
            Err(error) => vec![Effect::Notice(words::cannot_transfer(error.kind()))],
        }
    }

    /// [`AppState::transfer`] of the active pane's items into the folder the other pane shows,
    /// asking about every conflict: what F5 and F6 do in a debug build run with
    /// `RF_DEBUG_DIRECT_TRANSFER=1`, until the destination picker comes.
    pub fn transfer_to_other_pane(&mut self, mode: TransferMode) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        let side = self.active;
        let dest = self.tab(RIGHT - side).location().clone();
        self.transfer(side, mode, &dest, TransferOptions::default())
    }

    /// The window over the main window, if one shows.
    pub fn modal(&self) -> Option<&Modal> {
        self.modal.as_ref()
    }

    /// The rows of the job panel.
    pub fn job_rows(&self) -> Vec<JobRow> {
        self.jobs.rows()
    }

    /// The number of jobs that have not ended.
    pub fn active_jobs(&self) -> usize {
        self.jobs.active()
    }

    /// Whether Undo can start now: there is something to undo, and no undo or redo is under
    /// way (one runs at a time).
    pub fn can_undo(&self) -> bool {
        self.history.top.is_some() && !self.jobs.undo_running()
    }

    /// Whether Redo can start now.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo && !self.jobs.undo_running()
    }

    /// Whether the app is ending.
    pub fn quitting(&self) -> bool {
        self.quitting
    }

    /// Follows the job the engine started for an [`Effect::Job`].
    pub fn track(&mut self, started: Started) {
        self.jobs.track(started);
    }

    /// Takes what the engine's history can undo and redo next.
    pub fn set_history(&mut self, history: History) {
        self.history = history;
    }

    /// Takes an event of the job engine: the job panel follows it, a job that ended is told of
    /// (a toast, its new item selected, the items the trash refused offered for a permanent
    /// delete), and the jobs the last run left unfinished are offered again.
    pub fn job_event(&mut self, event: JobEvent) -> Vec<Effect> {
        match event {
            JobEvent::Recovered { jobs, .. } => {
                if !jobs.is_empty() && !self.quitting {
                    self.open(Modal::Recovery(Recovery { jobs }));
                }
                Vec::new()
            }
            JobEvent::JournalError { .. } => {
                if self.journal_error_told || self.quitting {
                    return Vec::new();
                }
                self.journal_error_told = true;
                vec![Effect::Notice(words::journal_error().to_owned())]
            }
            event => match self.jobs.apply(event) {
                Some(ended) => self.job_ended(ended),
                None => Vec::new(),
            },
        }
    }

    /// Takes a question of a job: it shows once the windows before it have closed. While the
    /// app ends, it is left unanswered, for the release that follows the engine's shutdown.
    pub fn question(&mut self, question: Question) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        let Question { id, job, asked } = question;
        let modal = match asked {
            Asked::Conflict(conflict) => Modal::Conflict(ConflictQuestion {
                id,
                job,
                conflict,
                items_left: self.jobs.items_left(job),
            }),
            Asked::Preflight(issues) => Modal::Preflight(PreflightQuestion { id, job, issues }),
        };
        self.open(modal);
        Vec::new()
    }

    /// The name in the name window changed to `text`.
    pub fn name_edited(&mut self, text: &str) {
        if let Some(Modal::Name(dialog)) = &mut self.modal {
            text.clone_into(&mut dialog.text);
        }
    }

    /// The name window was confirmed with `text`: its job is submitted and it closes. A name
    /// that cannot be used keeps it open with its warning, and the name the item has already
    /// closes it with nothing to do.
    pub fn name_confirmed(&mut self, text: &str) -> Vec<Effect> {
        let Some(Modal::Name(dialog)) = &mut self.modal else {
            return Vec::new();
        };
        text.clone_into(&mut dialog.text);
        match (dialog.check(), dialog.job()) {
            (_, Ok(Some((spec, origin)))) => {
                self.close();
                vec![submit(spec, Some(origin))]
            }
            (NameCheck::Unchanged, _) => {
                self.close();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The window that shows was closed without its main button (Esc, Cancel, the close
    /// button): a job question is answered with its cancelling choice, which stops the job.
    pub fn dialog_cancelled(&mut self) -> Vec<Effect> {
        let effects = match &self.modal {
            Some(Modal::Conflict(question)) => vec![Effect::Job(JobEffect::AnswerConflict(
                question.id,
                rf_jobs::ConflictAnswer::once(rf_jobs::ConflictAction::Cancel),
            ))],
            Some(Modal::Preflight(question)) => vec![Effect::Job(JobEffect::AnswerPreflight(
                question.id,
                rf_jobs::PreflightAnswer::Cancel,
            ))],
            _ => Vec::new(),
        };
        self.close();
        effects
    }

    /// The confirmation that shows was accepted: its job is submitted, or the app ends.
    pub fn confirm_accepted(&mut self) -> Vec<Effect> {
        let Some(Modal::Confirm(confirm)) = &self.modal else {
            return Vec::new();
        };
        match (&confirm.purpose, confirm.job()) {
            (ConfirmPurpose::Quit, _) => self.begin_quit(),
            (_, Some(spec)) => {
                self.close();
                vec![submit(spec, None)]
            }
            (_, None) => {
                self.close();
                Vec::new()
            }
        }
    }

    /// The conflict window was answered with `choice`, for the remaining conflicts of the job
    /// too when `apply_all`. An overwrite that is not offered does nothing.
    pub fn conflict_answered(&mut self, choice: ConflictChoice, apply_all: bool) -> Vec<Effect> {
        let Some(Modal::Conflict(question)) = &self.modal else {
            return Vec::new();
        };
        let Some(answer) = question.answer(choice, apply_all) else {
            return Vec::new();
        };
        let id = question.id;
        self.close();
        vec![Effect::Job(JobEffect::AnswerConflict(id, answer))]
    }

    /// The pre-flight window was answered with button `choice`
    /// ([`PreflightQuestion::answer`]).
    pub fn preflight_answered(&mut self, choice: i32) -> Vec<Effect> {
        let Some(Modal::Preflight(question)) = &self.modal else {
            return Vec::new();
        };
        let id = question.id;
        self.close();
        vec![Effect::Job(JobEffect::AnswerPreflight(
            id,
            PreflightQuestion::answer(choice),
        ))]
    }

    /// Recovered job `index` of the recovery window is run again (`run_again`) or dropped. The
    /// window closes with its last job.
    pub fn recovery_chosen(&mut self, index: usize, run_again: bool) -> Vec<Effect> {
        let Some(Modal::Recovery(recovery)) = &mut self.modal else {
            return Vec::new();
        };
        if index >= recovery.jobs.len() {
            return Vec::new();
        }
        let job = recovery.jobs.remove(index);
        if recovery.jobs.is_empty() {
            self.close();
        }
        let effect = if run_again {
            JobEffect::Resubmit {
                id: job.id,
                spec: job.spec,
            }
        } else {
            JobEffect::Dismiss(job.id)
        };
        vec![Effect::Job(effect)]
    }

    /// The pause (`0`), resume (`1`) or cancel (`2`) button of row `row` of the job panel.
    pub fn job_control(&mut self, row: usize, button: i32) -> Vec<Effect> {
        let Some(id) = self.jobs.row_id(row) else {
            return Vec::new();
        };
        let effect = match button {
            0 => JobEffect::Pause(id),
            1 => JobEffect::Resume(id),
            _ => JobEffect::Cancel(id),
        };
        vec![Effect::Job(effect)]
    }

    /// The button of the toast `token` was clicked. An undo starts only while the toast's job
    /// is the next one the history undoes; otherwise a notice says why not, so that the button
    /// never undoes another job.
    pub fn toast_clicked(&mut self, token: u64) -> Vec<Effect> {
        let Some((shown, _)) = &self.toast else {
            return Vec::new();
        };
        if *shown != token || self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        let Some((_, action)) = self.toast.take() else {
            return Vec::new();
        };
        match action {
            ToastAction::Undo(record) => {
                if self.history.top.as_ref() == Some(&record) && self.can_undo() {
                    vec![Effect::Job(JobEffect::Undo)]
                } else {
                    vec![Effect::Notice(words::undo_order().to_owned())]
                }
            }
            ToastAction::Details(failures) => {
                self.open(Modal::Failures(failures));
                Vec::new()
            }
        }
    }

    /// The toast `token` closed (its close button, or its time ran out).
    pub fn toast_closed(&mut self, token: u64) {
        if self
            .toast
            .as_ref()
            .is_some_and(|(shown, _)| *shown == token)
        {
            self.toast = None;
        }
    }

    /// The window's close button: with jobs under way, a confirmation first (over any window
    /// that shows, which comes back if the user stays); without, the app ends.
    pub fn request_close(&mut self) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        let running = self.jobs.active();
        if running == 0 {
            return self.begin_quit();
        }
        if let Some(Modal::Confirm(confirm)) = &self.modal
            && confirm.purpose == ConfirmPurpose::Quit
        {
            return Vec::new();
        }
        if let Some(shown) = self.modal.take() {
            self.later.push_front(shown);
        }
        self.modal = Some(Modal::Confirm(Confirm::quit(running)));
        Vec::new()
    }

    /// The app ends: every window closes, no new one opens, and the questions still open stay
    /// unanswered here. [`Effect::Quit`] then shuts the engine down before it releases them
    /// (carry-m1-03 "quit order").
    pub fn begin_quit(&mut self) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        self.quitting = true;
        self.modal = None;
        self.later.clear();
        self.toast = None;
        vec![Effect::Quit]
    }

    /// Whether the location of `paths` has a trash, as the worker found
    /// ([`Effect::CheckTrash`]): with one they go to it; without, a confirmation asks before
    /// they are deleted for good, which never happens without it (OPS-03).
    pub fn trash_checked(&mut self, paths: Vec<VPath>, supported: bool) -> Vec<Effect> {
        if paths.is_empty() || self.quitting {
            return Vec::new();
        }
        if supported {
            return vec![submit(
                JobSpec::Delete(DeleteSpec {
                    paths,
                    method: DeleteMethod::Trash,
                    errors: ErrorPolicy::Continue,
                }),
                None,
            )];
        }
        self.open(Modal::Confirm(Confirm::delete_permanently(
            paths,
            DeleteWhy::NoTrash,
        )));
        Vec::new()
    }

    /// The folders `folders` changed through a job of this window (a create, a rename, a
    /// delete, a transfer, an undo). The panes follow their folders' watches; this is where
    /// the folder tree will refresh the nodes of these folders it has listed (a follow-up of
    /// the shared folder tree). It asks for nothing yet.
    pub fn folders_changed(&mut self, folders: &[VPath]) -> Vec<Effect> {
        let _ = folders;
        Vec::new()
    }

    /// Tells of the job that ended: see [`AppState::job_event`].
    fn job_ended(&mut self, ended: Ended) -> Vec<Effect> {
        let Ended {
            work,
            origin,
            report,
            ..
        } = ended;
        let mut effects = self.folders_changed(&touched_folders(&report.spec, &report.outputs));
        if self.quitting || report.outcome == Outcome::Cancelled {
            return effects;
        }
        if report.outcome == Outcome::Completed
            && matches!(work, JobWork::Create | JobWork::Rename)
            && let (Some(side), Some(output)) = (origin, report.outputs.first())
        {
            effects.extend(self.select_output(side, output));
        }

        // The items the trash refused are still there; only the user may delete them for good.
        let refused: Vec<VPath> = report
            .failures
            .iter()
            .filter(|failure| failure.reason == Some(FailureReason::TrashRefused))
            .filter_map(|failure| failure.source.clone())
            .collect();
        if !refused.is_empty() {
            self.open(Modal::Confirm(Confirm::delete_permanently(
                refused,
                DeleteWhy::TrashRefused,
            )));
        }
        let failures: Vec<&ItemFailure> = report
            .failures
            .iter()
            .filter(|failure| failure.reason != Some(FailureReason::TrashRefused))
            .collect();
        if let Some(first) = failures.first() {
            if matches!(work, JobWork::Create | JobWork::Rename) {
                effects.push(Effect::Notice(words::name_refused(work, first)));
            } else {
                let lines = failures.iter().map(|failure| words::failure_line(failure));
                let failures_shown = Failures {
                    title: words::failures_title().to_owned(),
                    lines: lines.collect(),
                };
                effects.push(self.toast(
                    words::items_failed(failures.len()),
                    Some(ToastAction::Details(failures_shown)),
                ));
            }
            return effects;
        }
        if !report.failures.is_empty() {
            // Only refusals of the trash, which the confirmation tells of.
            return effects;
        }
        match (report.outcome, work) {
            (Outcome::Completed, JobWork::Copy | JobWork::Move | JobWork::Trash) => {
                let work = if report.copied_instead_of_moved {
                    JobWork::Copy
                } else {
                    work
                };
                let text = words::items_done(work, top_level_count(&report.spec), report.skipped);
                let action = report.undo.clone().map(ToastAction::Undo);
                effects.push(self.toast(text, action));
            }
            (Outcome::Completed, JobWork::Undo) => {
                if let JobSpec::Undo(record) = &report.spec {
                    effects.push(self.toast(words::undone(record), None));
                }
            }
            (Outcome::Completed, JobWork::Redo) => {
                effects.push(self.toast(words::outcome(Outcome::Completed, work), None));
            }
            (Outcome::Aborted, _) => {
                effects.push(self.toast(words::outcome(Outcome::Aborted, work), None));
            }
            _ => {}
        }
        effects
    }

    /// Selects `output`, the item a create or a rename made, in pane `side` if it still shows
    /// the item's folder: now if the row is there, else once it comes.
    fn select_output(&mut self, side: usize, output: &VPath) -> Vec<Effect> {
        let (Some(folder), Some(name)) = (output.parent(), output.file_name()) else {
            return Vec::new();
        };
        let tab = self.tab_mut(side);
        if *tab.location() != folder {
            return Vec::new();
        }
        tab.select_name(name.to_os_string())
            .map(|row| Effect::ScrollTo { side, row })
            .into_iter()
            .collect()
    }

    /// A toast with `text`, whose button does `action`, replacing the one on screen.
    fn toast(&mut self, text: String, action: Option<ToastAction>) -> Effect {
        self.toast_token += 1;
        let token = self.toast_token;
        let label = match &action {
            Some(ToastAction::Undo(_)) => words::undo_action(),
            Some(ToastAction::Details(_)) => words::details_action(),
            None => "",
        };
        self.toast = action.map(|action| (token, action));
        Effect::Toast {
            token,
            text,
            action: label.to_owned(),
        }
    }

    /// Shows `modal` now, or after the windows before it have closed. Nothing opens while the
    /// app ends.
    fn open(&mut self, modal: Modal) {
        if self.quitting {
            return;
        }
        if self.modal.is_none() {
            self.modal = Some(modal);
        } else {
            self.later.push_back(modal);
        }
    }

    /// Closes the window that shows; the next one waiting shows instead.
    fn close(&mut self) {
        self.modal = self.later.pop_front();
    }

    /// The items a command acts on in pane `side`: the selected rows, or the cursor's row
    /// when none is selected.
    fn targets(&self, side: usize) -> Vec<VPath> {
        let tab = self.tab(side);
        let mut rows = tab.selection().indices(tab.visible());
        if rows.is_empty() {
            rows.extend(
                tab.selection()
                    .cursor()
                    .filter(|&row| row < tab.visible().len()),
            );
        }
        rows.into_iter()
            .filter_map(|row| tab.location().join(&tab.visible()[row].name).ok())
            .collect()
    }

    /// F7, Ctrl+Shift+N: the name window for a new folder in the active tab's folder.
    fn new_folder(&mut self, side: usize) -> Vec<Effect> {
        let parent = self.tab(side).location().clone();
        self.open(Modal::Name(NameDialog::new_folder(parent, side)));
        Vec::new()
    }

    /// F2: the name window for the cursor's row (or the selected row without a cursor).
    fn rename(&mut self, side: usize) -> Vec<Effect> {
        let tab = self.tab(side);
        let row = tab
            .selection()
            .cursor()
            .filter(|&row| row < tab.visible().len())
            .or_else(|| tab.selection().indices(tab.visible()).first().copied());
        let Some(entry) = row.and_then(|row| tab.visible().get(row)) else {
            return Vec::new();
        };
        let is_dir = entry.kind == EntryKind::Dir;
        let Ok(path) = tab.location().join(&entry.name) else {
            return Vec::new();
        };
        if let Some(dialog) = NameDialog::rename(path, is_dir, side) {
            self.open(Modal::Name(dialog));
        }
        Vec::new()
    }

    /// Delete: the items go to the trash once a worker has found that their folder has one
    /// ([`Effect::CheckTrash`]).
    fn trash(&mut self, side: usize) -> Vec<Effect> {
        let paths = self.targets(side);
        if paths.is_empty() {
            return Vec::new();
        }
        let folder = self.tab(side).location().clone();
        vec![Effect::CheckTrash { folder, paths }]
    }

    /// Shift+Delete: a confirmation before the items are deleted for good.
    fn delete_permanently(&mut self, side: usize) -> Vec<Effect> {
        let paths = self.targets(side);
        if !paths.is_empty() {
            self.open(Modal::Confirm(Confirm::delete_permanently(
                paths,
                DeleteWhy::Asked,
            )));
        }
        Vec::new()
    }

    /// Ctrl+Z, or the toolbar's button: the last job is undone, when one can be. The button
    /// of a toast on screen does nothing from then on: the history has moved.
    fn undo(&mut self) -> Vec<Effect> {
        if self.can_undo() {
            self.toast = None;
            vec![Effect::Job(JobEffect::Undo)]
        } else {
            Vec::new()
        }
    }

    /// Ctrl+Y, Ctrl+Shift+Z: the last undo is done again, when one can be.
    fn redo(&mut self) -> Vec<Effect> {
        if self.can_redo() {
            self.toast = None;
            vec![Effect::Job(JobEffect::Redo)]
        } else {
            Vec::new()
        }
    }

    /// A click on row `row` of pane `side`: Ctrl flips the row, Shift selects the range from
    /// the anchor, a plain click selects the row alone. The pane gets the keyboard.
    pub fn click_row(&mut self, side: usize, row: usize, ctrl: bool, shift: bool) -> Vec<Effect> {
        self.activate(side);
        let (selection, rows) = self.tab_mut(side).selection_mut();
        if shift {
            selection.extend_to(row, rows);
        } else if ctrl {
            selection.toggle(row, rows);
        } else {
            selection.click(row, rows);
        }
        Vec::new()
    }

    /// Opens row `row` of pane `side` (Enter or a double click): a folder shows in the tab, a
    /// file opens in its app, and a link is followed first.
    pub fn open_row(&mut self, side: usize, row: usize) -> Vec<Effect> {
        self.activate(side);
        let tab = self.tab_mut(side);
        match tab.open_target(row) {
            Some(OpenAction::Navigate(folder)) => vec![Effect::Load(tab.navigate(folder))],
            Some(OpenAction::OpenFile(file)) => vec![Effect::OpenFile(file)],
            Some(OpenAction::Follow(link)) => vec![Effect::Follow {
                generation: tab.generation(),
                link,
            }],
            None => Vec::new(),
        }
    }

    /// Shows tab `index` of pane `side`.
    pub fn select_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        self.panes[side].select_tab(index);
        Vec::new()
    }

    /// Closes tab `index` of pane `side`, unless it is the pane's last one.
    pub fn close_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        // Its load stops once the controller sees its generation is gone.
        drop(self.panes[side].close_tab(index));
        Vec::new()
    }

    /// Opens a tab in pane `side` at the folder the pane shows.
    pub fn add_tab(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        let here = self.tab(side).location().clone();
        vec![Effect::Load(self.panes[side].add_tab(here))]
    }

    /// Goes to piece `index` of the path row of pane `side` (0 is the root).
    pub fn open_crumb(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        let Some(folder) = ancestors(self.tab(side).location()).into_iter().nth(index) else {
            return Vec::new();
        };
        vec![Effect::Load(self.tab_mut(side).navigate(folder))]
    }

    /// Goes to the path typed into the path row of pane `side`. Text that is no path gives
    /// a notice and changes nothing.
    pub fn enter_path(&mut self, side: usize, text: &str) -> Vec<Effect> {
        self.activate(side);
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        match VPath::parse(text) {
            Ok(folder) => vec![Effect::Load(self.tab_mut(side).navigate(folder))],
            Err(_) => vec![Effect::Notice(words::cannot_open(ErrorKind::InvalidPath))],
        }
    }

    /// Sorts pane `side` by `key`, the other way round when it already sorts by it.
    pub fn sort(&mut self, side: usize, key: SortKey) -> Vec<Effect> {
        self.activate(side);
        self.tab_mut(side).set_sort(key);
        Vec::new()
    }

    /// Lists the folder of pane `side` again ("다시 시도").
    pub fn retry(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        vec![Effect::Load(self.tab_mut(side).reload())]
    }

    /// Shows `folder` in the active pane.
    pub fn go_to(&mut self, folder: VPath) -> Vec<Effect> {
        let side = self.active;
        vec![Effect::Load(self.tab_mut(side).navigate(folder))]
    }

    /// Makes `places` (the favorites, then the drives) the roots of the folder tree, which
    /// starts again from them, closed, and finds the folder it follows again.
    pub fn set_places(&mut self, places: &[Place]) -> Vec<Effect> {
        let roots = places
            .iter()
            .map(|place| (place.label.clone(), place.path.clone()));
        self.with_tree(|tree| tree.set_roots(roots))
    }

    /// Shows the folder of row `row` of the tree in the active pane (a click on the row). The
    /// row is the tree's selection from then on, even when another row shows the same folder.
    pub fn open_tree_row(&mut self, row: usize) -> Vec<Effect> {
        let Some(id) = self.tree.node_at(row) else {
            return Vec::new();
        };
        let Some(folder) = self.tree.node(id).map(|node| node.path.clone()) else {
            return Vec::new();
        };
        self.tree.select(id);
        self.go_to(folder)
    }

    /// Opens or closes row `row` of the tree (a click on its chevron); the first opening
    /// lists the folder.
    pub fn toggle_tree_row(&mut self, row: usize) -> Vec<Effect> {
        match self.tree.node_at(row) {
            Some(id) => self.with_tree(|tree| tree.toggle(id).into_iter().collect()),
            None => Vec::new(),
        }
    }

    /// Makes the tree follow the active pane (LAY-03 a): when the folder the active pane
    /// shows is not the one the tree follows (the pane, its tab or its folder changed), the
    /// tree opens the folders on the way to it and selects it. The controller calls this
    /// after every change; it does nothing while the folder stays the same, so a folder the
    /// user closed in the tree stays closed.
    pub fn follow_active(&mut self) -> Vec<Effect> {
        let location = self.tab(self.active).location();
        if self.tree.target() == Some(location) {
            return Vec::new();
        }
        let location = location.clone();
        self.with_tree(|tree| tree.reveal(&location))
    }

    /// Takes an event of the loader. A rescan lists the folder again; a vanished folder is
    /// looked for on a worker thread. The ticket of a vanished folder stays: its listing, if
    /// still running, still comes. An event that no tab asked for goes to the folder tree.
    pub fn loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        let Some(side) = self.side_of(event.generation()) else {
            return self.tree_loaded(event);
        };
        match self.panes[side].handle(event) {
            Handled::Stale => Vec::new(),
            // A row a job made may have come: it was selected, and shows.
            Handled::Applied { tab } => {
                let shows = tab == self.panes[side].active();
                self.panes[side].tabs_mut()[tab]
                    .take_revealed()
                    .filter(|_| shows)
                    .map(|row| Effect::ScrollTo { side, row })
                    .into_iter()
                    .collect()
            }
            Handled::Reload { request, .. } => vec![Effect::Load(request)],
            Handled::Vanished {
                generation, path, ..
            } => vec![Effect::FindExisting { generation, path }],
        }
    }

    /// Takes where the tab with `generation`, whose folder `path` vanished, can go: `found`,
    /// the nearest folder of `path` that is still there, or home when none is. A tab that has
    /// moved on since takes nothing. When `found` is `path` itself (the folder was replaced,
    /// or only its watch failed), the folder is listed again; within [`AUTO_RELOAD_GAP`] of
    /// its last automatic reload, that waits for the gap to pass ([`Effect::ReloadLater`]),
    /// so a watch that keeps failing cannot reload it in a loop and a folder replaced twice
    /// in quick succession still shows the second one. Otherwise the tab goes to `found`.
    pub fn found_existing(
        &mut self,
        generation: u64,
        path: &VPath,
        found: Option<VPath>,
        now: Instant,
    ) -> Vec<Effect> {
        let Some(target) = found.or_else(|| self.home.clone()) else {
            return Vec::new();
        };
        if target == *path {
            if self.tab_by_generation(generation).is_none() {
                return Vec::new();
            }
            let since = self
                .auto_reloads
                .get(path)
                .map(|&at| now.saturating_duration_since(at))
                .filter(|&since| since < AUTO_RELOAD_GAP);
            if let Some(since) = since {
                if !self.pending_reloads.insert(generation) {
                    return Vec::new();
                }
                return vec![Effect::ReloadLater {
                    generation,
                    path: path.clone(),
                    after: AUTO_RELOAD_GAP.saturating_sub(since),
                }];
            }
            return self.auto_reload(generation, path, now);
        }
        match self.tab_by_generation(generation) {
            Some(tab) => vec![Effect::Load(tab.navigate(target))],
            None => Vec::new(),
        }
    }

    /// The time of the reload [`Effect::ReloadLater`] asked for has come: the tab with
    /// `generation` lists its folder `path` again, unless it has moved on since.
    pub fn reload_due(&mut self, generation: u64, path: &VPath, now: Instant) -> Vec<Effect> {
        if !self.pending_reloads.remove(&generation) {
            return Vec::new();
        }
        self.auto_reload(generation, path, now)
    }

    /// Lists the folder `path` of the tab with `generation` again after its watch ended, and
    /// notes when; nothing for a tab that has moved on.
    fn auto_reload(&mut self, generation: u64, path: &VPath, now: Instant) -> Vec<Effect> {
        self.pending_reloads.remove(&generation);
        let Some(tab) = self.tab_by_generation(generation) else {
            return Vec::new();
        };
        let request = tab.reload();
        self.auto_reloads
            .retain(|_, &mut at| now.saturating_duration_since(at) < AUTO_RELOAD_GAP);
        self.auto_reloads.insert(path.clone(), now);
        vec![Effect::Load(request)]
    }

    /// Takes what following a link gave ([`Loader::follow`](crate::loader::Loader::follow)).
    /// Only the tab that still has the generation the result carries takes it: a tab that has
    /// moved on since takes nothing.
    pub fn followed(&mut self, followed: Followed) -> Vec<Effect> {
        let Followed { generation, result } = followed;
        let Some(tab) = self.tab_by_generation(generation) else {
            return Vec::new();
        };
        match result {
            Ok(OpenAction::Navigate(folder)) => vec![Effect::Load(tab.navigate(folder))],
            Ok(OpenAction::OpenFile(file)) => vec![Effect::OpenFile(file)],
            // Following never gives another link to follow.
            Ok(OpenAction::Follow(_)) => Vec::new(),
            Err(kind) => vec![Effect::Notice(words::cannot_open(kind))],
        }
    }

    /// Takes an event of a listing of the folder tree; one that no node waits for is dropped.
    /// A listing that failed leaves the node empty, with no notice.
    fn tree_loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        match event {
            LoadEvent::Listed {
                generation,
                entries,
            } => self.with_tree(|tree| tree.apply_children(generation, entries)),
            LoadEvent::Failed { generation, .. } => {
                self.tree.listing_failed(generation);
                Vec::new()
            }
            // A listing of folders sends nothing else.
            LoadEvent::Changes { .. }
            | LoadEvent::Rescan { .. }
            | LoadEvent::Vanished { .. }
            | LoadEvent::Space { .. } => Vec::new(),
        }
    }

    /// Changes the tree with `f` and turns what it asks for into effects: the listings to
    /// start, and a scroll to the row of the active pane's folder when the tree has just
    /// found it.
    fn with_tree(&mut self, f: impl FnOnce(&mut Tree) -> Vec<LoadRequest>) -> Vec<Effect> {
        let before = self.tree.selected();
        let mut effects: Vec<Effect> = f(&mut self.tree)
            .into_iter()
            .map(Effect::LoadChildren)
            .collect();
        if self.tree.selected() != before
            && let Some(row) = self.tree.selected_row()
        {
            effects.push(Effect::ScrollTree(row));
        }
        effects
    }

    fn tab(&self, side: usize) -> &Tab {
        self.panes[side].active_tab()
    }

    fn tab_mut(&mut self, side: usize) -> &mut Tab {
        self.panes[side].active_tab_mut()
    }

    fn tab_by_generation(&mut self, generation: u64) -> Option<&mut Tab> {
        self.panes
            .iter_mut()
            .find_map(|pane| pane.tab_by_generation_mut(generation))
    }

    /// The pane one of whose tabs made the request `generation`.
    fn side_of(&self, generation: u64) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| pane.tabs().iter().any(|tab| tab.generation() == generation))
    }

    /// Moves the active tab of pane `side` with `step` (back, forward, up), if it can go.
    fn go(&mut self, side: usize, step: fn(&mut Tab) -> Option<LoadRequest>) -> Vec<Effect> {
        step(self.tab_mut(side))
            .map(Effect::Load)
            .into_iter()
            .collect()
    }

    fn move_cursor(&mut self, side: usize, delta: isize, extend: bool) -> Vec<Effect> {
        let (selection, rows) = self.tab_mut(side).selection_mut();
        selection.move_cursor(delta, extend, rows);
        selection
            .cursor()
            .map(|row| Effect::ScrollTo { side, row })
            .into_iter()
            .collect()
    }

    /// Shows or hides the hidden items in every tab of both panes, and the hidden folders of
    /// the tree.
    fn toggle_hidden(&mut self) -> Vec<Effect> {
        self.show_hidden = !self.show_hidden;
        for pane in &mut self.panes {
            for tab in pane.tabs_mut() {
                tab.set_show_hidden(self.show_hidden);
            }
        }
        let show = self.show_hidden;
        self.with_tree(|tree| tree.set_show_hidden(show))
    }
}

/// The effect that submits `spec`, asked for in pane `origin`.
fn submit(spec: JobSpec, origin: Option<usize>) -> Effect {
    Effect::Job(JobEffect::Submit { spec, origin })
}

/// The number of items a job was asked to act on at its top level: the items of a transfer,
/// the paths of a delete, and one for the others.
fn top_level_count(spec: &JobSpec) -> usize {
    match spec {
        JobSpec::Transfer(transfer) => transfer.items.len(),
        JobSpec::Delete(delete) => delete.paths.len(),
        JobSpec::Undo(record) => record.item_count(),
        _ => 1,
    }
}

/// The folders a job of `spec` changed, with `outputs`, the paths it made: the folders of its
/// sources and targets, each once.
fn touched_folders(spec: &JobSpec, outputs: &[VPath]) -> Vec<VPath> {
    let mut paths: Vec<&VPath> = outputs.iter().collect();
    match spec {
        JobSpec::Transfer(transfer) => {
            for item in &transfer.items {
                paths.push(&item.source);
                paths.push(&item.target);
            }
        }
        JobSpec::Delete(delete) => paths.extend(&delete.paths),
        JobSpec::Rename { from, to } => paths.extend([from, to]),
        // A create's output is the item it made, and an undo's what it put back; a kind of
        // job added after this was written has its outputs.
        _ => {}
    }
    let mut folders: Vec<VPath> = Vec::new();
    for folder in paths.into_iter().filter_map(VPath::parent) {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders
}

/// The folders the panes start at: the left one at `start` (made absolute against
/// `current_dir`) or else home, the right one at the documents folder or else home. Without
/// a home folder, `root` stands in. `None` when there is no folder at all.
pub fn start_folders(
    start: Option<&Path>,
    current_dir: Option<&Path>,
    home: Option<&VPath>,
    documents: Option<&VPath>,
    root: Option<&VPath>,
) -> Option<(VPath, VPath)> {
    let fallback = home.or(root)?;
    let left = start
        .and_then(|path| match current_dir {
            Some(dir) if path.is_relative() => VPath::local(dir.join(path)).ok(),
            _ => VPath::local(path).ok(),
        })
        .unwrap_or_else(|| fallback.clone());
    let right = documents.unwrap_or(fallback).clone();
    Some((left, right))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rf_core::{Attributes, Entry, EntryKind};
    use rf_jobs::{JobId, JobReport};
    use rf_vfs::SortOrder;
    use slint::platform::Key;

    use super::*;
    use crate::keys;
    use crate::places::PlaceKind;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    fn file(name: &str) -> Entry {
        Entry::new(name, EntryKind::File)
    }

    fn dir(name: &str) -> Entry {
        Entry::new(name, EntryKind::Dir)
    }

    fn loads(effects: &[Effect]) -> Vec<&LoadRequest> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Load(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    /// The one request among `effects`.
    fn the_load(effects: &[Effect]) -> LoadRequest {
        match loads(effects).as_slice() {
            [request] => (*request).clone(),
            other => panic!("expected one load, got {other:?} in {effects:?}"),
        }
    }

    /// A window state whose left pane at `a` lists `left` and whose right pane at `b` lists
    /// `right`.
    fn ready(left: Vec<Entry>, right: Vec<Entry>) -> AppState {
        let (mut state, effects) = AppState::new(folder("a"), folder("b"), Some(folder("home")));
        let requests = loads(&effects);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, folder("a"));
        assert_eq!(requests[1].path, folder("b"));
        for (request, entries) in requests.into_iter().zip([left, right]) {
            let generation = request.generation;
            let effects = state.loaded(LoadEvent::Listed {
                generation,
                entries,
            });
            assert_eq!(effects, []);
        }
        state
    }

    fn rows(state: &AppState, side: usize) -> Vec<&str> {
        state.panes()[side]
            .active_tab()
            .visible()
            .iter()
            .map(|entry| entry.name.to_str().expect("test names are text"))
            .collect()
    }

    fn selected(state: &AppState, side: usize) -> Vec<usize> {
        let tab = state.panes()[side].active_tab();
        tab.selection().indices(tab.visible())
    }

    fn cursor(state: &AppState, side: usize) -> Option<usize> {
        state.panes()[side].active_tab().selection().cursor()
    }

    fn press(state: &mut AppState, text: &str, mods: Mods) -> Option<Vec<Effect>> {
        let command = keys::command_for(text, mods).expect("a shortcut");
        state.key(command, mods, 10, Instant::now())
    }

    fn special(key: Key) -> String {
        char::from(key).to_string()
    }

    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };

    #[test]
    fn startup_lists_both_panes() {
        let state = ready(vec![dir("x"), file("y")], vec![file("z")]);
        assert_eq!(state.active(), LEFT);
        assert_eq!(rows(&state, LEFT), ["x", "y"]);
        assert_eq!(rows(&state, RIGHT), ["z"]);
        assert_eq!(state.generations().len(), 2);
    }

    #[test]
    fn tab_switches_panes_and_clicks_activate() {
        let mut state = ready(vec![file("x")], vec![file("z")]);
        assert_eq!(press(&mut state, &special(Key::Tab), NONE), Some(vec![]));
        assert_eq!(state.active(), RIGHT);
        press(&mut state, &special(Key::Tab), SHIFT);
        assert_eq!(state.active(), LEFT);
        state.click_row(RIGHT, 0, false, false);
        assert_eq!(state.active(), RIGHT);
        assert_eq!(selected(&state, RIGHT), [0]);
    }

    #[test]
    fn keys_move_the_cursor_and_select() {
        let names = ["a", "b", "c", "d", "e"];
        let mut state = ready(names.iter().copied().map(file).collect(), vec![]);
        let down = special(Key::DownArrow);
        assert_eq!(
            press(&mut state, &down, NONE),
            Some(vec![Effect::ScrollTo { side: LEFT, row: 0 }])
        );
        press(&mut state, &down, NONE);
        assert_eq!(
            (cursor(&state, LEFT), selected(&state, LEFT)),
            (Some(1), vec![1])
        );
        // Shift extends from the anchor.
        press(&mut state, &down, SHIFT);
        press(&mut state, &down, SHIFT);
        assert_eq!(selected(&state, LEFT), [1, 2, 3]);
        // Ctrl+Space flips the cursor's row; Esc clears; Ctrl+A selects all.
        press(&mut state, &special(Key::Space), CTRL);
        assert_eq!(selected(&state, LEFT), [1, 2]);
        press(&mut state, &special(Key::Escape), NONE);
        assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
        assert_eq!(cursor(&state, LEFT), Some(3));
        press(&mut state, "a", CTRL);
        assert_eq!(selected(&state, LEFT), [0, 1, 2, 3, 4]);
        // Home, End and the page keys stop at the ends.
        press(&mut state, &special(Key::Home), NONE);
        assert_eq!(cursor(&state, LEFT), Some(0));
        assert_eq!(
            press(&mut state, &special(Key::PageDown), NONE),
            Some(vec![Effect::ScrollTo { side: LEFT, row: 4 }])
        );
        press(&mut state, &special(Key::End), SHIFT);
        assert_eq!(selected(&state, LEFT), [4]);
        // Clicks: Ctrl adds, Shift takes the range from the anchor.
        state.click_row(LEFT, 1, false, false);
        state.click_row(LEFT, 3, true, false);
        assert_eq!(selected(&state, LEFT), [1, 3]);
        state.click_row(LEFT, 4, false, true);
        assert_eq!(selected(&state, LEFT), [3, 4]);
    }

    #[test]
    fn typing_jumps_to_a_name() {
        let mut state = ready(vec![file("apple"), file("banana"), file("berry")], vec![]);
        let now = Instant::now();
        let effects = state.key(Command::TypeToSelect("be".into()), NONE, 10, now);
        assert_eq!(effects, Some(vec![Effect::ScrollTo { side: LEFT, row: 2 }]));
        assert_eq!(selected(&state, LEFT), [2]);
        // No match moves nothing, and the key is still used.
        let effects = state.key(
            Command::TypeToSelect("x".into()),
            NONE,
            10,
            now + Duration::from_secs(5),
        );
        assert_eq!(effects, Some(vec![]));
        assert_eq!(selected(&state, LEFT), [2]);
    }

    #[test]
    fn open_follows_the_row_kind() {
        let link = Entry::new("latest", EntryKind::Symlink);
        let mut state = ready(vec![dir("sub"), file("notes.txt"), link], vec![]);
        let generation = state.panes()[LEFT].active_tab().generation();

        assert_eq!(
            state.open_row(LEFT, 1),
            [Effect::OpenFile(
                folder("a").join("notes.txt").expect("a name")
            )]
        );
        assert_eq!(
            state.open_row(LEFT, 2),
            [Effect::Follow {
                generation,
                link: folder("a").join("latest").expect("a name"),
            }]
        );
        // Enter opens the cursor's row: the folder shows in the tab.
        state.click_row(LEFT, 0, false, false);
        let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a").join("sub").expect("a name"));
        assert_ne!(request.generation, generation);
        // Back and Up go to `a` again.
        let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("used");
        assert_eq!(the_load(&effects).path, folder("a"));
        let effects = press(&mut state, &special(Key::Backspace), NONE).expect("used");
        let temp = VPath::local(std::env::temp_dir()).expect("a local path");
        assert_eq!(the_load(&effects).path, temp);
    }

    #[test]
    fn a_late_link_result_is_dropped() {
        let mut state = ready(vec![Entry::new("latest", EntryKind::Symlink)], vec![]);
        let effects = state.open_row(LEFT, 0);
        let Effect::Follow { generation, link } = effects[0].clone() else {
            panic!("a link is followed: {effects:?}");
        };
        // The user moves on before the link's target is known.
        state.go_to(folder("elsewhere"));
        let late = state.followed(Followed {
            generation,
            result: Ok(OpenAction::Navigate(link.clone())),
        });
        assert_eq!(late, []);
        assert_eq!(
            state.panes()[LEFT].active_tab().location(),
            &folder("elsewhere")
        );

        // In time, a link to a folder shows it, one to a file opens it, and a broken one
        // gives a notice.
        let generation = state.panes()[LEFT].active_tab().generation();
        assert_eq!(
            state.followed(Followed {
                generation,
                result: Ok(OpenAction::OpenFile(link.clone())),
            }),
            [Effect::OpenFile(link.clone())]
        );
        assert_eq!(
            state.followed(Followed {
                generation,
                result: Err(ErrorKind::NotFound),
            }),
            [Effect::Notice(
                "열 수 없습니다: 찾을 수 없습니다".to_owned()
            )]
        );
        let effects = state.followed(Followed {
            generation,
            result: Ok(OpenAction::Navigate(link.clone())),
        });
        assert_eq!(the_load(&effects).path, link);
    }

    #[test]
    fn a_vanished_folder_goes_to_the_nearest_parent() {
        let mut state = ready(vec![file("x")], vec![]);
        let generation = state.panes()[LEFT].active_tab().generation();
        let effects = state.loaded(LoadEvent::Vanished { generation });
        assert_eq!(
            effects,
            [Effect::FindExisting {
                generation,
                path: folder("a"),
            }]
        );
        // The ticket stays until the tab moves on: a listing still running still comes.
        assert!(state.generations().contains(&generation));

        let parent = folder("a").parent().expect("a parent");
        let effects = state.found_existing(
            generation,
            &folder("a"),
            Some(parent.clone()),
            Instant::now(),
        );
        assert_eq!(the_load(&effects).path, parent);
        // A second answer for the same generation is late.
        assert_eq!(
            state.found_existing(generation, &folder("a"), Some(parent), Instant::now()),
            []
        );
    }

    #[test]
    fn a_replaced_folder_is_listed_again_but_not_in_a_loop() {
        let mut state = ready(vec![file("old")], vec![]);
        let now = Instant::now();
        let generation = state.panes()[LEFT].active_tab().generation();
        state.loaded(LoadEvent::Vanished { generation });
        let effects = state.found_existing(generation, &folder("a"), Some(folder("a")), now);
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a"));
        // A reload keeps the rows until the new listing comes.
        assert_eq!(rows(&state, LEFT), ["old"]);

        // The new watch ends at once again: no second reload within the gap, but one when it
        // has passed.
        state.loaded(LoadEvent::Vanished {
            generation: request.generation,
        });
        let soon = now + Duration::from_secs(1);
        assert_eq!(
            state.found_existing(request.generation, &folder("a"), Some(folder("a")), soon),
            [Effect::ReloadLater {
                generation: request.generation,
                path: folder("a"),
                after: AUTO_RELOAD_GAP.saturating_sub(Duration::from_secs(1)),
            }]
        );
        // An answer after the gap reloads at once, and the waiting reload then has nothing to do.
        let later = now + AUTO_RELOAD_GAP + Duration::from_secs(1);
        let effects =
            state.found_existing(request.generation, &folder("a"), Some(folder("a")), later);
        assert_eq!(the_load(&effects).path, folder("a"));
        assert_eq!(
            state.reload_due(request.generation, &folder("a"), later),
            []
        );
    }

    #[test]
    fn a_folder_replaced_twice_quickly_is_listed_once_more_after_the_gap() {
        let mut state = ready(vec![file("first")], vec![]);
        let now = Instant::now();
        let a = folder("a");
        // The first replacement reloads at once.
        let generation = state.panes()[LEFT].active_tab().generation();
        state.loaded(LoadEvent::Vanished { generation });
        let first = the_load(&state.found_existing(generation, &a, Some(a.clone()), now));

        // The second, a second later, waits for the rest of the gap; asking again adds nothing.
        state.loaded(LoadEvent::Vanished {
            generation: first.generation,
        });
        let soon = now + Duration::from_secs(1);
        let later = vec![Effect::ReloadLater {
            generation: first.generation,
            path: a.clone(),
            after: AUTO_RELOAD_GAP.saturating_sub(Duration::from_secs(1)),
        }];
        assert_eq!(
            state.found_existing(first.generation, &a, Some(a.clone()), soon),
            later
        );
        assert_eq!(
            state.found_existing(first.generation, &a, Some(a.clone()), soon),
            []
        );

        // When the gap has passed, exactly one more reload comes.
        let due = now + AUTO_RELOAD_GAP;
        let second = the_load(&state.reload_due(first.generation, &a, due));
        assert_eq!(second.path, a);
        assert_ne!(second.generation, first.generation);
        assert_eq!(state.reload_due(first.generation, &a, due), []);
        assert_eq!(
            state.panes()[LEFT].active_tab().generation(),
            second.generation
        );
    }

    #[test]
    fn a_waiting_reload_of_a_tab_that_moved_on_does_nothing() {
        let mut state = ready(vec![file("x")], vec![]);
        let now = Instant::now();
        let a = folder("a");
        let generation = state.panes()[LEFT].active_tab().generation();
        let first = the_load(&state.found_existing(generation, &a, Some(a.clone()), now));
        let soon = now + Duration::from_secs(1);
        let effects = state.found_existing(first.generation, &a, Some(a.clone()), soon);
        assert!(matches!(effects.as_slice(), [Effect::ReloadLater { .. }]));
        // The user opens another folder before the gap has passed.
        state.go_to(folder("elsewhere"));
        assert_eq!(
            state.reload_due(first.generation, &a, now + AUTO_RELOAD_GAP),
            []
        );
        assert_eq!(
            state.panes()[LEFT].active_tab().location(),
            &folder("elsewhere")
        );
    }

    #[test]
    fn a_folder_with_nothing_left_goes_home() {
        let mut state = ready(vec![], vec![]);
        let generation = state.panes()[RIGHT].active_tab().generation();
        let effects = state.found_existing(generation, &folder("b"), None, Instant::now());
        assert_eq!(the_load(&effects).path, folder("home"));
        assert_eq!(
            state.panes()[RIGHT].active_tab().location(),
            &folder("home")
        );
    }

    #[test]
    fn hidden_items_toggle_in_every_tab() {
        let mut dot = file(".dot");
        dot.attributes |= Attributes::HIDDEN;
        let mut state = ready(vec![dot.clone(), file("x")], vec![dot, file("y")]);
        assert_eq!(rows(&state, LEFT), ["x"]);
        assert_eq!(press(&mut state, "h", CTRL), Some(vec![]));
        assert!(state.show_hidden());
        assert_eq!(rows(&state, LEFT), [".dot", "x"]);
        assert_eq!(rows(&state, RIGHT), [".dot", "y"]);
        // A new tab takes the setting.
        let request = the_load(&press(&mut state, "t", CTRL).expect("used"));
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![{
                let mut dot = file(".new");
                dot.attributes |= Attributes::HIDDEN;
                dot
            }],
        });
        assert_eq!(rows(&state, LEFT), [".new"]);
        press(&mut state, "h", CTRL);
        assert_eq!(rows(&state, LEFT), Vec::<&str>::new());
    }

    #[test]
    fn tabs_open_show_and_close() {
        let mut state = ready(vec![file("x")], vec![]);
        let first = state.panes()[LEFT].active_tab().generation();
        let effects = state.add_tab(LEFT);
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a"));
        assert_eq!(state.panes()[LEFT].tabs().len(), 2);
        assert_eq!(state.panes()[LEFT].active(), 1);
        state.select_tab(LEFT, 0);
        assert_eq!(state.panes()[LEFT].active(), 0);
        // Ctrl+W closes the tab that shows; the last tab stays.
        press(&mut state, "w", CTRL);
        assert_eq!(state.panes()[LEFT].tabs().len(), 1);
        assert!(!state.generations().contains(&first));
        assert!(state.generations().contains(&request.generation));
        press(&mut state, "w", CTRL);
        assert_eq!(state.panes()[LEFT].tabs().len(), 1);
        // A click on the other pane's tab makes that pane active.
        state.close_tab(RIGHT, 0);
        assert_eq!(state.active(), RIGHT);
    }

    #[test]
    fn crumbs_and_typed_paths_navigate() {
        let mut state = ready(vec![], vec![]);
        let chain = ancestors(&folder("a"));
        assert_eq!(chain.last(), Some(&folder("a")));
        assert_eq!(chain[0].parent(), None);
        let effects = state.open_crumb(LEFT, 0);
        assert_eq!(the_load(&effects).path, chain[0]);
        assert_eq!(state.open_crumb(LEFT, 99), []);

        let typed = std::env::temp_dir().join("typed");
        let effects = state.enter_path(RIGHT, &format!("  {}  ", typed.display()));
        assert_eq!(the_load(&effects).path, folder("typed"));
        assert_eq!(state.active(), RIGHT);
        assert_eq!(state.enter_path(RIGHT, "   "), []);
        assert_eq!(
            state.enter_path(RIGHT, "relative/path"),
            [Effect::Notice(
                "열 수 없습니다: 쓸 수 없는 이름이나 경로입니다".to_owned()
            )]
        );
        assert_eq!(
            press(&mut state, "l", CTRL),
            Some(vec![Effect::EditPath(RIGHT)])
        );
    }

    #[test]
    fn sort_and_retry_act_on_the_clicked_pane() {
        // The loader sends a listing sorted by name.
        let mut state = ready(vec![file("a"), file("b")], vec![]);
        assert_eq!(rows(&state, LEFT), ["a", "b"]);
        state.sort(LEFT, SortKey::Name);
        assert_eq!(rows(&state, LEFT), ["b", "a"]);
        let sort = state.panes()[LEFT].active_tab().sort();
        assert_eq!(sort.primary.order, SortOrder::Descending);
        let effects = state.retry(RIGHT);
        assert_eq!(the_load(&effects).path, folder("b"));
        assert_eq!(state.active(), RIGHT);
    }

    #[test]
    fn copy_and_move_keys_wait_for_the_picker() {
        let mut state = ready(vec![file("x")], vec![]);
        state.click_row(LEFT, 0, false, false);
        for text in [special(Key::F5), special(Key::F6)] {
            assert_eq!(press(&mut state, &text, NONE), None, "{text:?}");
        }
        assert_eq!(state.modal(), None);
    }

    /// The one job `effects` submit, and the pane that asked for it.
    fn the_submit(effects: &[Effect]) -> (JobSpec, Option<usize>) {
        let submits: Vec<(JobSpec, Option<usize>)> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Job(JobEffect::Submit { spec, origin }) => Some((spec.clone(), *origin)),
                _ => None,
            })
            .collect();
        match submits.as_slice() {
            [one] => one.clone(),
            other => panic!("expected one job, got {other:?} in {effects:?}"),
        }
    }

    fn in_a(name: &str) -> VPath {
        folder("a").join(name).expect("a plain name")
    }

    fn delete(paths: Vec<VPath>, method: DeleteMethod) -> JobSpec {
        JobSpec::Delete(DeleteSpec {
            paths,
            method,
            errors: ErrorPolicy::Continue,
        })
    }

    #[test]
    fn new_folder_asks_for_a_name_then_creates_it() {
        let mut state = ready(vec![file("x")], vec![]);
        assert_eq!(press(&mut state, &special(Key::F7), NONE), Some(vec![]));
        let Some(Modal::Name(dialog)) = state.modal() else {
            panic!("a name dialog: {:?}", state.modal());
        };
        assert_eq!(dialog.initial, "새 폴더");
        // The dialog has the keys now: a shortcut that comes here anyway does nothing.
        assert_eq!(press(&mut state, &special(Key::Delete), NONE), Some(vec![]));
        // A name that cannot be one keeps the dialog open.
        state.name_edited("a/b");
        assert_eq!(state.name_confirmed("a/b"), []);
        assert!(matches!(state.modal(), Some(Modal::Name(_))));
        let effects = state.name_confirmed("자료");
        assert_eq!(
            the_submit(&effects),
            (
                JobSpec::Create {
                    parent: folder("a"),
                    name: "자료".into(),
                    kind: rf_jobs::CreateKind::Dir,
                },
                Some(LEFT)
            )
        );
        assert_eq!(state.modal(), None);
        // Ctrl+Shift+N opens it too, and Esc closes it without a job.
        press(
            &mut state,
            "N",
            Mods {
                shift: true,
                ..CTRL
            },
        );
        assert!(matches!(state.modal(), Some(Modal::Name(_))));
        assert_eq!(state.dialog_cancelled(), []);
        assert_eq!(state.modal(), None);
    }

    #[test]
    fn rename_starts_from_the_cursor_row() {
        let mut state = ready(vec![dir("docs"), file("설계안.pdf")], vec![]);
        // Nothing under the cursor: F2 does nothing.
        assert_eq!(press(&mut state, &special(Key::F2), NONE), Some(vec![]));
        assert_eq!(state.modal(), None);
        state.click_row(LEFT, 1, false, false);
        press(&mut state, &special(Key::F2), NONE);
        let Some(Modal::Name(dialog)) = state.modal() else {
            panic!("a name dialog");
        };
        assert_eq!(&dialog.initial[..dialog.select_end], "설계안");
        // The name it has closes the dialog with nothing to do.
        assert_eq!(state.name_confirmed("설계안.pdf"), []);
        assert_eq!(state.modal(), None);
        press(&mut state, &special(Key::F2), NONE);
        let effects = state.name_confirmed("최종안.pdf");
        assert_eq!(
            the_submit(&effects),
            (
                JobSpec::Rename {
                    from: in_a("설계안.pdf"),
                    to: in_a("최종안.pdf"),
                },
                Some(LEFT)
            )
        );
    }

    #[test]
    fn delete_checks_for_a_trash_and_never_deletes_for_good_unasked() {
        let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
        state.click_row(LEFT, 0, false, false);
        state.click_row(LEFT, 1, true, false);
        let paths = vec![in_a("a.txt"), in_a("b.txt")];
        let effects = press(&mut state, &special(Key::Delete), NONE).expect("used");
        assert_eq!(
            effects,
            [Effect::CheckTrash {
                folder: folder("a"),
                paths: paths.clone(),
            }]
        );
        // With a trash, the items go there.
        assert_eq!(
            the_submit(&state.trash_checked(paths.clone(), true)).0,
            delete(paths.clone(), DeleteMethod::Trash)
        );
        // Without one, nothing is submitted until the user says yes (OPS-03).
        assert_eq!(state.trash_checked(paths.clone(), false), []);
        let Some(Modal::Confirm(confirm)) = state.modal() else {
            panic!("a confirmation");
        };
        assert_eq!(
            confirm.message,
            "휴지통을 쓸 수 없는 위치입니다. 항목 2개를 영구 삭제할까요? 되돌릴 수 없습니다."
        );
        assert!(confirm.danger);
        // No keeps the items.
        assert_eq!(state.dialog_cancelled(), []);
        assert_eq!(state.modal(), None);
        state.trash_checked(paths.clone(), false);
        assert_eq!(
            the_submit(&state.confirm_accepted()).0,
            delete(paths.clone(), DeleteMethod::Permanent)
        );
        assert_eq!(state.modal(), None);

        // Shift+Delete asks first too.
        assert_eq!(
            press(&mut state, &special(Key::Delete), SHIFT),
            Some(vec![])
        );
        let Some(Modal::Confirm(confirm)) = state.modal() else {
            panic!("a confirmation");
        };
        assert_eq!(
            confirm.message,
            "항목 2개를 영구 삭제할까요? 휴지통을 거치지 않으며 되돌릴 수 없습니다."
        );
        assert_eq!(confirm.purpose, ConfirmPurpose::DeletePermanently(paths));
    }

    /// Ends the job `id` that `spec` describes and the state followed as `work`, with the
    /// report `edit` makes, and gives what the state asks for.
    fn finish(
        state: &mut AppState,
        id: u64,
        spec: &JobSpec,
        work: JobWork,
        edit: impl FnOnce(&mut JobReport),
    ) -> Vec<Effect> {
        state.track(Started {
            id: JobId(id),
            spec: Some(Arc::new(spec.clone())),
            work,
            origin: Some(LEFT),
            via_history: matches!(work, JobWork::Undo | JobWork::Redo),
        });
        let mut report = JobReport::new(JobId(id), spec);
        edit(&mut report);
        state.job_event(JobEvent::Finished {
            id: JobId(id),
            report: Arc::new(report),
        })
    }

    fn failure(source: VPath, kind: ErrorKind, reason: Option<FailureReason>) -> ItemFailure {
        ItemFailure {
            source: Some(source),
            target: None,
            stage: rf_jobs::Stage::Delete,
            kind,
            message: String::new(),
            reason,
        }
    }

    #[test]
    fn items_the_trash_refused_are_offered_for_a_permanent_delete() {
        let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
        let spec = delete(vec![in_a("a.txt"), in_a("b.txt")], DeleteMethod::Trash);
        let effects = finish(&mut state, 1, &spec, JobWork::Trash, |report| {
            report.outcome = Outcome::CompletedWithErrors;
            report.failures.push(failure(
                in_a("b.txt"),
                ErrorKind::Unsupported,
                Some(FailureReason::TrashRefused),
            ));
        });
        // Nothing is deleted yet, and the confirmation is all that tells of the refusal.
        assert!(
            effects
                .iter()
                .all(|effect| !matches!(effect, Effect::Job(_) | Effect::Toast { .. })),
            "{effects:?}"
        );
        let Some(Modal::Confirm(confirm)) = state.modal() else {
            panic!("a confirmation");
        };
        assert_eq!(
            confirm.purpose,
            ConfirmPurpose::DeletePermanently(vec![in_a("b.txt")])
        );
        assert!(
            confirm
                .message
                .starts_with("휴지통이 받지 않은 항목이 1개 있습니다"),
            "{}",
            confirm.message
        );
        assert_eq!(
            the_submit(&state.confirm_accepted()).0,
            delete(vec![in_a("b.txt")], DeleteMethod::Permanent)
        );
    }

    fn moved(from: &str) -> UndoRecord {
        UndoRecord::Moved {
            moves: vec![rf_jobs::MovedItem {
                from: in_a(from),
                to: folder("b").join(from).expect("a plain name"),
            }],
            created_dirs: Vec::new(),
            removed_dirs: Vec::new(),
            overwritten: 0,
            copied_dirs: Vec::new(),
        }
    }

    /// The token, text and button of the one toast among `effects`.
    fn the_toast(effects: &[Effect]) -> (u64, String, String) {
        let toasts: Vec<(u64, String, String)> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Toast {
                    token,
                    text,
                    action,
                } => Some((*token, text.clone(), action.clone())),
                _ => None,
            })
            .collect();
        match toasts.as_slice() {
            [one] => one.clone(),
            other => panic!("expected one toast, got {other:?} in {effects:?}"),
        }
    }

    #[test]
    fn a_finished_move_offers_its_undo_while_it_is_next() {
        let mut state = ready(vec![file("a.txt")], vec![]);
        // Nothing to move without a selection or a cursor.
        let options = TransferOptions::default();
        assert_eq!(
            state.transfer(LEFT, TransferMode::Move, &folder("b"), options),
            []
        );
        state.click_row(LEFT, 0, false, false);
        let effects = state.transfer(LEFT, TransferMode::Move, &folder("b"), options);
        let (spec, origin) = the_submit(&effects);
        assert_eq!(origin, Some(LEFT));
        let record = moved("a.txt");
        // The engine notes the record before it tells of the end.
        state.set_history(History {
            top: Some(record.clone()),
            can_redo: false,
        });
        let effects = finish(&mut state, 1, &spec, JobWork::Move, |report| {
            report.undo = Some(record.clone());
        });
        let (token, text, action) = the_toast(&effects);
        assert_eq!(
            (text.as_str(), action.as_str()),
            ("항목 1개를 이동했습니다", "되돌리기")
        );
        assert!(state.can_undo());
        assert_eq!(state.toast_clicked(token), [Effect::Job(JobEffect::Undo)]);
        // The toast's button works once.
        assert_eq!(state.toast_clicked(token), []);

        // An undo from the keyboard leaves the toast's button with nothing to do.
        let effects = finish(&mut state, 3, &spec, JobWork::Move, |report| {
            report.undo = Some(record.clone());
        });
        let (token, _, _) = the_toast(&effects);
        assert_eq!(
            press(&mut state, "z", CTRL),
            Some(vec![Effect::Job(JobEffect::Undo)])
        );
        assert_eq!(state.toast_clicked(token), []);

        // A later job is the next to undo: the toast of the move does not undo it.
        let effects = finish(&mut state, 2, &spec, JobWork::Move, |report| {
            report.undo = Some(record.clone());
        });
        let (token, _, _) = the_toast(&effects);
        state.set_history(History {
            top: Some(UndoRecord::Renamed {
                from: in_a("x"),
                to: in_a("y"),
            }),
            can_redo: false,
        });
        assert_eq!(
            state.toast_clicked(token),
            [Effect::Notice(
                "나중에 한 작업이 있어 먼저 그 작업을 되돌려야 합니다".to_owned()
            )]
        );
    }

    #[test]
    fn undo_is_off_while_an_undo_runs() {
        let mut state = ready(vec![], vec![]);
        // Nothing to undo: Ctrl+Z does nothing.
        assert!(!state.can_undo());
        assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
        let record = moved("a.txt");
        state.set_history(History {
            top: Some(record.clone()),
            can_redo: false,
        });
        assert_eq!(
            press(&mut state, "z", CTRL),
            Some(vec![Effect::Job(JobEffect::Undo)])
        );
        // The engine starts the undo; until it ends, neither undo nor redo starts another.
        state.track(Started {
            id: JobId(9),
            spec: None,
            work: JobWork::Undo,
            origin: None,
            via_history: true,
        });
        state.set_history(History {
            top: None,
            can_redo: false,
        });
        assert!(!state.can_undo() && !state.can_redo());
        assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
        assert_eq!(press(&mut state, "y", CTRL), Some(vec![]));
        // It ends: the toast names what it undid, and the redo can start.
        state.set_history(History {
            top: None,
            can_redo: true,
        });
        let spec = JobSpec::Undo(record);
        let mut report = JobReport::new(JobId(9), &spec);
        report.outcome = Outcome::Completed;
        let effects = state.job_event(JobEvent::Finished {
            id: JobId(9),
            report: Arc::new(report),
        });
        assert_eq!(the_toast(&effects).1, "이동을 되돌렸습니다");
        assert!(state.can_redo());
        assert_eq!(
            press(
                &mut state,
                "z",
                Mods {
                    shift: true,
                    ..CTRL
                }
            ),
            Some(vec![Effect::Job(JobEffect::Redo)])
        );
    }

    fn conflict_question(id: u64) -> Question {
        Question {
            id,
            job: JobId(1),
            asked: Asked::Conflict(Box::new(rf_jobs::Conflict {
                source: folder("b").join("a.txt").expect("a name"),
                target: in_a("a.txt"),
                incoming: Entry::new("a.txt", EntryKind::File),
                existing: Entry::new("a.txt", EntryKind::File),
            })),
        }
    }

    #[test]
    fn questions_wait_behind_the_open_dialog_and_answer_the_job() {
        let mut state = ready(vec![file("x")], vec![]);
        press(&mut state, &special(Key::F7), NONE);
        assert_eq!(state.question(conflict_question(5)), []);
        assert!(matches!(state.modal(), Some(Modal::Name(_))));
        state.dialog_cancelled();
        let Some(Modal::Conflict(question)) = state.modal() else {
            panic!("the conflict shows next");
        };
        assert_eq!(question.id, 5);
        assert_eq!(
            state.conflict_answered(ConflictChoice::Overwrite, true),
            [Effect::Job(JobEffect::AnswerConflict(
                5,
                rf_jobs::ConflictAnswer::all(rf_jobs::ConflictAction::Overwrite)
            ))]
        );
        assert_eq!(state.modal(), None);

        // Esc stops the job of a question: its cancelling answer.
        state.question(conflict_question(7));
        assert_eq!(
            state.dialog_cancelled(),
            [Effect::Job(JobEffect::AnswerConflict(
                7,
                rf_jobs::ConflictAnswer::once(rf_jobs::ConflictAction::Cancel)
            ))]
        );
        let preflight = Question {
            id: 8,
            job: JobId(2),
            asked: Asked::Preflight(Vec::new()),
        };
        state.question(preflight.clone());
        assert_eq!(
            state.preflight_answered(1),
            [Effect::Job(JobEffect::AnswerPreflight(
                8,
                rf_jobs::PreflightAnswer::SkipBlocked
            ))]
        );
        state.question(preflight);
        assert_eq!(
            state.dialog_cancelled(),
            [Effect::Job(JobEffect::AnswerPreflight(
                8,
                rf_jobs::PreflightAnswer::Cancel
            ))]
        );
    }

    #[test]
    fn closing_with_jobs_asks_first_then_leaves_questions_for_the_release() {
        // Without a job the app ends at once.
        let mut idle = ready(vec![], vec![]);
        assert_eq!(idle.request_close(), [Effect::Quit]);
        assert!(idle.quitting());
        assert_eq!(idle.request_close(), []);

        let mut state = ready(vec![file("x")], vec![]);
        let spec = delete(vec![in_a("x")], DeleteMethod::Trash);
        state.track(Started {
            id: JobId(1),
            spec: Some(Arc::new(spec)),
            work: JobWork::Trash,
            origin: None,
            via_history: false,
        });
        state.question(conflict_question(3));
        // The confirmation shows over the question.
        assert_eq!(state.request_close(), []);
        let Some(Modal::Confirm(confirm)) = state.modal() else {
            panic!("a confirmation");
        };
        assert_eq!(confirm.purpose, ConfirmPurpose::Quit);
        assert_eq!(
            confirm.message,
            "진행 중인 작업 1개가 있습니다. 끝내면 다음 실행 때 이어서 할 수 있습니다."
        );
        // Staying brings the question back, still unanswered.
        assert_eq!(state.dialog_cancelled(), []);
        assert!(matches!(state.modal(), Some(Modal::Conflict(q)) if q.id == 3));
        state.request_close();
        assert_eq!(state.confirm_accepted(), [Effect::Quit]);
        assert!(state.quitting());
        assert_eq!(state.modal(), None);
        // From now on nothing shows and nothing is answered here: the engine shuts down first,
        // then the release answers the open questions (carry-m1-03 "quit order").
        assert_eq!(state.question(conflict_question(4)), []);
        assert_eq!(state.modal(), None);
        assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
        assert_eq!(state.request_close(), []);
    }

    fn incomplete(id: u64) -> rf_jobs::IncompleteJob {
        rf_jobs::IncompleteJob {
            id: JobId(id),
            spec: delete(vec![in_a("x")], DeleteMethod::Trash),
            submitted: std::time::SystemTime::now(),
            started: true,
            temp_dirs: Vec::new(),
        }
    }

    #[test]
    fn unfinished_jobs_are_offered_again() {
        let mut state = ready(vec![], vec![]);
        let effects = state.job_event(JobEvent::Recovered {
            jobs: Vec::new(),
            temp_files_removed: 0,
        });
        assert_eq!(effects, []);
        assert_eq!(state.modal(), None);
        state.job_event(JobEvent::Recovered {
            jobs: vec![incomplete(1), incomplete(2)],
            temp_files_removed: 3,
        });
        assert!(matches!(state.modal(), Some(Modal::Recovery(r)) if r.jobs.len() == 2));
        assert_eq!(
            state.recovery_chosen(0, true),
            [Effect::Job(JobEffect::Resubmit {
                id: JobId(1),
                spec: incomplete(1).spec,
            })]
        );
        assert!(matches!(state.modal(), Some(Modal::Recovery(r)) if r.jobs.len() == 1));
        assert_eq!(state.recovery_chosen(5, false), []);
        assert_eq!(
            state.recovery_chosen(0, false),
            [Effect::Job(JobEffect::Dismiss(JobId(2)))]
        );
        assert_eq!(state.modal(), None);
    }

    #[test]
    fn the_new_folder_is_selected_when_it_shows() {
        let mut state = ready(vec![file("x")], vec![]);
        let spec = JobSpec::Create {
            parent: folder("a"),
            name: "새 폴더".into(),
            kind: rf_jobs::CreateKind::Dir,
        };
        let effects = finish(&mut state, 1, &spec, JobWork::Create, |report| {
            report.outputs.push(in_a("새 폴더"));
        });
        // Not listed yet: nothing to scroll to, and no toast for a create.
        assert!(effects.is_empty(), "{effects:?}");
        let generation = state.panes()[LEFT].active_tab().generation();
        let effects = state.loaded(LoadEvent::Changes {
            generation,
            changes: vec![rf_vfs::Change::Upsert(dir("새 폴더"))],
        });
        assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 0 }]);
        assert_eq!(rows(&state, LEFT), ["새 폴더", "x"]);
        assert_eq!(selected(&state, LEFT), [0]);
        assert_eq!(cursor(&state, LEFT), Some(0));

        // A rename that is listed already is selected at once.
        let spec = JobSpec::Rename {
            from: in_a("x"),
            to: in_a("x"),
        };
        let effects = finish(&mut state, 2, &spec, JobWork::Rename, |report| {
            report.outputs.push(in_a("x"));
        });
        assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 1 }]);
        assert_eq!(selected(&state, LEFT), [1]);
    }

    #[test]
    fn a_failed_rename_says_why_and_a_failed_copy_lists_its_items() {
        let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
        let spec = JobSpec::Rename {
            from: in_a("a.txt"),
            to: in_a("b.txt"),
        };
        let effects = finish(&mut state, 1, &spec, JobWork::Rename, |report| {
            report.outcome = Outcome::CompletedWithErrors;
            report
                .failures
                .push(failure(in_a("a.txt"), ErrorKind::AlreadyExists, None));
        });
        assert_eq!(
            effects,
            [Effect::Notice(
                "이름을 바꾸지 못했습니다: 같은 이름의 항목이 이미 있습니다".to_owned()
            )]
        );

        let spec = JobSpec::Transfer(
            TransferSpec::into_dir(
                TransferMode::Copy,
                &[in_a("a.txt"), in_a("b.txt")],
                &folder("b"),
                TransferOptions::default(),
            )
            .expect("a transfer"),
        );
        let effects = finish(&mut state, 2, &spec, JobWork::Copy, |report| {
            report.outcome = Outcome::CompletedWithErrors;
            report
                .failures
                .push(failure(in_a("a.txt"), ErrorKind::PermissionDenied, None));
            report.failures.push(failure(
                in_a("b.txt"),
                ErrorKind::Unsupported,
                Some(FailureReason::NotARegularFile),
            ));
        });
        let (token, text, action) = the_toast(&effects);
        assert_eq!(
            (text.as_str(), action.as_str()),
            ("항목 2개를 처리하지 못했습니다", "자세히")
        );
        assert_eq!(state.toast_clicked(token), []);
        let Some(Modal::Failures(failures)) = state.modal() else {
            panic!("the failed items");
        };
        assert_eq!(failures.title, "처리하지 못한 항목");
        assert_eq!(
            failures.lines,
            [
                format!("{}: 권한이 없습니다", in_a("a.txt")),
                format!(
                    "{}: 장치나 파이프 같은 특수 파일이라 건너뛰었습니다",
                    in_a("b.txt")
                ),
            ]
        );
        // A job the user cancelled tells nothing.
        state.dialog_cancelled();
        let effects = finish(&mut state, 3, &spec, JobWork::Copy, |report| {
            report.outcome = Outcome::Cancelled;
        });
        assert_eq!(effects, []);
    }

    #[test]
    fn the_job_panel_buttons_reach_their_job() {
        let mut state = ready(vec![], vec![]);
        assert_eq!(state.job_control(0, 0), []);
        let spec = delete(vec![in_a("x")], DeleteMethod::Trash);
        state.track(Started {
            id: JobId(4),
            spec: Some(Arc::new(spec)),
            work: JobWork::Trash,
            origin: None,
            via_history: false,
        });
        state.job_event(JobEvent::State {
            id: JobId(4),
            state: rf_jobs::JobState::Running,
        });
        assert_eq!(state.job_rows().len(), 1);
        assert_eq!(
            state.job_control(0, 0),
            [Effect::Job(JobEffect::Pause(JobId(4)))]
        );
        assert_eq!(
            state.job_control(0, 1),
            [Effect::Job(JobEffect::Resume(JobId(4)))]
        );
        assert_eq!(
            state.job_control(0, 2),
            [Effect::Job(JobEffect::Cancel(JobId(4)))]
        );
    }

    #[test]
    fn a_right_click_keeps_a_selection_it_falls_in() {
        let mut state = ready(vec![file("a"), file("b"), file("c")], vec![]);
        state.click_row(LEFT, 0, false, false);
        state.click_row(LEFT, 1, true, false);
        state.row_menu(RIGHT, 0);
        assert_eq!(state.active(), RIGHT);
        state.row_menu(LEFT, 0);
        assert_eq!(state.active(), LEFT);
        assert_eq!(selected(&state, LEFT), [0, 1]);
        assert_eq!(cursor(&state, LEFT), Some(0));
        state.row_menu(LEFT, 2);
        assert_eq!(selected(&state, LEFT), [2]);
        assert_eq!(cursor(&state, LEFT), Some(2));
    }

    #[test]
    fn late_events_of_a_closed_tab_are_dropped() {
        let mut state = ready(vec![file("x")], vec![]);
        let request = the_load(&state.add_tab(LEFT));
        state.close_tab(LEFT, 1);
        let effects = state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![file("late")],
        });
        assert_eq!(effects, []);
        assert_eq!(rows(&state, LEFT), ["x"]);
        assert_eq!(
            state.found_existing(request.generation, &folder("a"), None, Instant::now()),
            []
        );
    }

    fn temp() -> VPath {
        VPath::local(std::env::temp_dir()).expect("a local path")
    }

    fn drive(label: &str, path: &VPath) -> Place {
        Place {
            label: label.to_owned(),
            path: path.clone(),
            kind: PlaceKind::Drive,
            space: None,
        }
    }

    /// The one listing of the folder tree among `effects`.
    fn the_tree_load(effects: &[Effect]) -> LoadRequest {
        let requests: Vec<&LoadRequest> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::LoadChildren(request) => Some(request),
                _ => None,
            })
            .collect();
        match requests.as_slice() {
            [request] => (*request).clone(),
            other => panic!("expected one tree listing, got {other:?} in {effects:?}"),
        }
    }

    fn tree_labels(state: &AppState) -> Vec<String> {
        state
            .tree()
            .rows()
            .into_iter()
            .map(|row| row.label)
            .collect()
    }

    #[test]
    fn the_tree_follows_the_active_pane() {
        let mut state = ready(vec![], vec![]);
        // The places come; the tree lists the place that holds the left pane's folder.
        let mut effects = state.set_places(&[drive("임시", &temp())]);
        effects.extend(state.follow_active());
        let request = the_tree_load(&effects);
        assert_eq!(request.path, temp());
        assert!(state.generations().contains(&request.generation));
        assert_eq!(state.tree().selected_row(), None);

        // Its listing selects the pane's folder and scrolls the tree to it.
        let effects = state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![dir("a"), dir("b"), dir("c"), file("x.txt")],
        });
        assert_eq!(effects, [Effect::ScrollTree(1)]);
        assert_eq!(tree_labels(&state), ["임시", "a", "b", "c"]);
        assert!(!state.generations().contains(&request.generation));
        // Nothing to do while the pane stays where it is.
        assert_eq!(state.follow_active(), []);

        // The right pane gets the keyboard: the tree selects its folder.
        press(&mut state, &special(Key::Tab), NONE);
        assert_eq!(state.follow_active(), [Effect::ScrollTree(2)]);
        // A click on a row shows its folder in the active pane; the row is selected already.
        let effects = state.open_tree_row(3);
        assert_eq!(the_load(&effects).path, folder("c"));
        assert_eq!(state.active(), RIGHT);
        assert_eq!(state.tree().selected_row(), Some(3));
        assert_eq!(state.follow_active(), []);
        // A folder under no place selects nothing and lists nothing.
        let elsewhere =
            VPath::new(rf_core::Scheme::Remote, "host", &["share"]).expect("a remote address");
        state.go_to(elsewhere);
        assert_eq!(state.follow_active(), []);
        assert_eq!(state.tree().selected(), None);
        assert_eq!(state.open_tree_row(99), []);
    }

    #[test]
    fn tree_rows_open_once_and_follow_the_hidden_setting() {
        let mut state = ready(vec![], vec![]);
        assert_eq!(state.set_places(&[drive("임시", &temp())]), []);
        let request = the_tree_load(&state.toggle_tree_row(0));
        assert_eq!(request.path, temp());
        // Closing and opening again asks for nothing more.
        assert_eq!(state.toggle_tree_row(0), []);
        assert_eq!(state.toggle_tree_row(0), []);
        assert_eq!(state.toggle_tree_row(99), []);
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![
                {
                    let mut dot = dir(".h");
                    dot.attributes |= Attributes::HIDDEN;
                    dot
                },
                dir("a"),
            ],
        });
        assert_eq!(tree_labels(&state), ["임시", "a"]);
        // Ctrl+H shows the hidden folders in the tree too.
        assert_eq!(press(&mut state, "h", CTRL), Some(vec![]));
        assert_eq!(tree_labels(&state), ["임시", ".h", "a"]);

        // A listing that failed leaves the row empty, without a notice.
        let a = the_tree_load(&state.toggle_tree_row(2));
        let effects = state.loaded(LoadEvent::Failed {
            generation: a.generation,
            kind: ErrorKind::PermissionDenied,
        });
        assert_eq!(effects, []);
        assert!(!state.tree().rows()[2].has_children);
        assert!(!state.generations().contains(&a.generation));
    }

    #[test]
    fn start_folders_fall_back_to_home() {
        let (home, docs, root) = (folder("home"), folder("docs"), folder("root"));
        let cwd = std::env::temp_dir();
        assert_eq!(
            start_folders(None, Some(&cwd), Some(&home), Some(&docs), Some(&root)),
            Some((home.clone(), docs.clone()))
        );
        assert_eq!(
            start_folders(
                Some(Path::new("rel")),
                Some(&cwd),
                Some(&home),
                None,
                Some(&root)
            ),
            Some((folder("rel"), home.clone()))
        );
        let absolute = std::env::temp_dir().join("abs");
        assert_eq!(
            start_folders(Some(&absolute), None, Some(&home), None, None),
            Some((folder("abs"), home))
        );
        assert_eq!(
            start_folders(None, None, None, None, Some(&root)),
            Some((root.clone(), root))
        );
        assert_eq!(start_folders(None, None, None, None, None), None);
    }
}
