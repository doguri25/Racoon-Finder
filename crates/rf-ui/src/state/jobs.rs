//! What the window asks of the job engine and what it makes of the engine's news: the file
//! commands, the dialogs over the main window, the job panel, the toasts, the trash check and
//! the app's end. A child of [`state`](super), so these methods see the fields of
//! [`AppState`].

use rf_core::{EntryKind, VPath};
use rf_jobs::{
    DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobEvent, JobReport,
    JobSpec, Outcome, TransferMode, TransferOptions, TransferSpec, UndoRecord,
};

use super::{AppState, Effect, RIGHT, ToastAction};
use crate::dialogs::{
    Confirm, ConfirmPurpose, ConflictChoice, ConflictQuestion, DeleteWhy, Failures, Modal,
    NameCheck, NameDialog, NamePurpose, PreflightQuestion, Recovery,
};
use crate::jobs::{Ended, History, JobEffect, JobRow, Started};
use crate::loader::LoadRequest;
use crate::pane::{LoadStatus, Pane, Tab};
use crate::resolver::{Asked, Question};
use crate::words::{self, JobWork};

impl AppState {
    /// Copies or moves the items of pane `side` (the selected rows, or the cursor's row) into
    /// the folder `dest`, with `options`, at once: for a target already known, without the
    /// destination picker. Nothing for a pane with no item to act on.
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
                work: self.jobs.work(job).unwrap_or(JobWork::Other),
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
    /// closes it with nothing to do. The destination picker under a window for its 새 폴더
    /// shows again, and waits for the folder.
    pub fn name_confirmed(&mut self, text: &str) -> Vec<Effect> {
        let Some(Modal::Name(dialog)) = &mut self.modal else {
            return Vec::new();
        };
        text.clone_into(&mut dialog.text);
        let for_picker = matches!(dialog.purpose, NamePurpose::PickFolder { .. });
        match (dialog.check(), dialog.job()) {
            (_, Ok(Some((spec, origin)))) => {
                self.close();
                if for_picker
                    && let JobSpec::Create { parent, name, .. } = &spec
                    && let Some(Modal::Pick(picker)) = &mut self.modal
                {
                    picker.making = Some((parent.clone(), name.clone()));
                }
                vec![submit(spec, origin)]
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

    /// The confirmation that shows was accepted: its job is submitted, or the app ends. The
    /// copies an undo could not trash are deleted only once a worker has found them and their
    /// originals as they were once more ([`AppState::copies_checked`]).
    pub fn confirm_accepted(&mut self) -> Vec<Effect> {
        let Some(Modal::Confirm(confirm)) = &self.modal else {
            return Vec::new();
        };
        match &confirm.purpose {
            ConfirmPurpose::Quit => self.begin_quit(),
            // No job of its own (`Confirm::job`): a worker looks at the copies first.
            ConfirmPurpose::DeleteCopies {
                copies,
                originals,
                record,
            } => {
                let (copies, originals) = (copies.clone(), originals.clone());
                let record = UndoRecord::clone(record);
                self.close();
                self.copies_confirmed(record, copies, originals)
            }
            ConfirmPurpose::DeletePermanently(_) => {
                let spec = confirm.job();
                self.close();
                spec.map(|spec| submit(spec, None)).into_iter().collect()
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
            // The window hides the button when it would not undo; a click that came first is
            // told why.
            ToastAction::Undo(_) if self.jobs.undo_running() => {
                vec![Effect::Notice(words::undo_busy().to_owned())]
            }
            ToastAction::Undo(record) if self.history.top.as_ref() != Some(&record) => {
                vec![Effect::Notice(words::undo_order().to_owned())]
            }
            ToastAction::Undo(_) => vec![Effect::Job(JobEffect::Undo)],
            ToastAction::Details(failures) => {
                self.open(Modal::Failures(failures));
                Vec::new()
            }
        }
    }

    /// Whether the button of the toast on screen still does what it says. The window hides it
    /// once it does not: an undo whose job is no longer the next one to undo, or while an undo
    /// or a redo runs; and any button once the history has moved on from the toast (an undo
    /// from the keyboard drops the toast's action).
    pub fn toast_button_live(&self) -> bool {
        match &self.toast {
            None => false,
            Some((_, ToastAction::Undo(record))) => {
                !self.jobs.undo_running() && self.history.top.as_ref() == Some(record)
            }
            Some((_, ToastAction::Details(_))) => true,
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
        self.drop_closed_picker();
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
    /// delete, a transfer, an undo). The panes follow their folders' watches, and the tree
    /// follows a folder a pane shows through that pane's listing ([`AppState::loaded`]); the
    /// tree's other listed nodes of these folders are listed again.
    pub fn folders_changed(&mut self, folders: &[VPath]) -> Vec<Effect> {
        let shown: Vec<&VPath> = self
            .panes
            .iter()
            .flat_map(Pane::tabs)
            .filter(|tab| tab.status() == LoadStatus::Ready)
            .map(Tab::location)
            .collect();
        let mut requests: Vec<LoadRequest> = Vec::new();
        for folder in folders.iter().filter(|folder| !shown.contains(folder)) {
            requests.extend(self.tree.relist_folder(folder));
            requests.extend(self.pick_tree.relist_folder(folder));
        }
        requests.into_iter().map(Effect::LoadChildren).collect()
    }

    /// Tells of the job that ended: see [`AppState::job_event`].
    fn job_ended(&mut self, ended: Ended) -> Vec<Effect> {
        let Ended {
            work,
            origin,
            report,
            ..
        } = ended;
        let touched = touched_folders(&report.spec, &report.outputs);
        let mut effects = self.folders_changed(&touched);
        effects.extend(self.pick_made(&report));
        // Whatever it did, a cancelled job too, it may have changed the room left.
        if !self.quitting {
            effects.push(self.space_to_read(&touched));
        }
        // Before the end of a cancelled copy, which may have made some of its items.
        self.note_copies(&report);
        if let Some(done) = self.copies_deleted(&report) {
            effects.extend(done);
            return effects;
        }
        if self.quitting || report.outcome == Outcome::Cancelled {
            return effects;
        }
        if report.outcome == Outcome::Completed
            && matches!(work, JobWork::Create | JobWork::Rename)
            && let (Some(side), Some(output)) = (origin, report.outputs.first())
        {
            effects.extend(self.select_output(side, output));
        }
        // What an undo or a redo brought back (a name, a folder, an item from the trash) is
        // selected where a pane shows its folder, the active one first.
        if matches!(work, JobWork::Undo | JobWork::Redo)
            && let Some(output) = report.outputs.first()
        {
            let side = [self.active, RIGHT - self.active]
                .into_iter()
                .find(|&side| Some(self.tab(side).location().clone()) == output.parent());
            if let Some(side) = side {
                effects.extend(self.select_output(side, output));
            }
        }

        // A trash job offers to delete for good what its trash refused, and so does the undo of
        // a copy, for the copies it could not trash, once their originals are found as they
        // were ([`AppState::copies_refused`]). No other undo does: an undo never deletes
        // anything for good itself (the undo contract of rf-jobs), and the refusals of an undo
        // of another record are failures like any other.
        let copied = match &report.spec {
            JobSpec::Undo(record @ UndoRecord::Copied { .. }) => Some(record),
            _ => None,
        };
        let refusal = |failure: &&ItemFailure| {
            (work == JobWork::Trash || copied.is_some())
                && failure.reason == Some(FailureReason::TrashRefused)
        };
        let refused: Vec<VPath> = report
            .failures
            .iter()
            .filter(refusal)
            .filter_map(|failure| failure.source.clone())
            .collect();
        let failures: Vec<&ItemFailure> = report
            .failures
            .iter()
            .filter(|failure| !refusal(failure))
            .collect();
        if let Some(record) = copied.filter(|_| !refused.is_empty()) {
            let record = record.clone();
            effects.extend(self.copies_refused(&record, refused, &failures, &report));
            return effects;
        }
        if !failures.is_empty() {
            effects.push(self.failure_notice(work, &failures, &report));
        } else if work == JobWork::Trash {
            effects.extend(self.trash_toast(&report));
        } else {
            effects.extend(self.done_toast(work, &report));
        }
        // The items the trash refused are still there; only the user may delete them for good,
        // after what did go to the trash has been told.
        if !refused.is_empty() {
            self.open(Modal::Confirm(Confirm::delete_permanently(
                refused,
                DeleteWhy::TrashRefused,
            )));
        }
        effects
    }

    /// The notice of a job some items of which failed: why, for a rename or a create (one
    /// item); for the others a toast whose button lists the items and why, and after them the
    /// items of `report` the location deleted for good instead of trashing them.
    fn failure_notice(
        &mut self,
        work: JobWork,
        failures: &[&ItemFailure],
        report: &JobReport,
    ) -> Effect {
        if matches!(work, JobWork::Create | JobWork::Rename) {
            return Effect::Notice(words::name_refused(work, failures[0]));
        }
        let gone: Vec<&ItemFailure> = deleted_for_good(report).collect();
        let shown = Failures {
            title: words::failures_title().to_owned(),
            lines: failures
                .iter()
                .chain(&gone)
                .map(|failure| words::failure_line(failure))
                .collect(),
        };
        self.toast(
            words::and_gone(words::items_failed(failures.len()), gone.len()),
            Some(ToastAction::Details(shown)),
        )
    }

    /// The toast of a trash job without failures other than refusals: what went to the trash,
    /// which its button brings back, and what the location deleted for good instead, which
    /// nothing brings back (a `DeletedNotTrashed` warning).
    fn trash_toast(&mut self, report: &JobReport) -> Option<Effect> {
        let trashed = report.undo.as_ref().map_or(0, UndoRecord::item_count);
        let gone = deleted_for_good(report).count();
        if trashed == 0 && gone == 0 {
            return (report.outcome == Outcome::Aborted)
                .then(|| self.toast(words::outcome(Outcome::Aborted, JobWork::Trash), None));
        }
        let action = report
            .undo
            .clone()
            .filter(|_| trashed > 0)
            .map(ToastAction::Undo);
        Some(self.toast(words::trash_done(trashed, gone), action))
    }

    /// The toast of a job other than a trash that ended without failures. An undo or a redo
    /// trashes too (what it takes back): it also tells what the location deleted for good
    /// instead.
    fn done_toast(&mut self, work: JobWork, report: &JobReport) -> Option<Effect> {
        let (text, action) = match (report.outcome, work) {
            (Outcome::Completed, JobWork::Copy | JobWork::Move) => {
                let work = if report.copied_instead_of_moved {
                    JobWork::Copy
                } else {
                    work
                };
                let done = report.done.files.saturating_add(report.done.dirs);
                let count = top_level_count(&report.spec);
                let text = words::items_done(work, count, done, report.skipped);
                (text, report.undo.clone().map(ToastAction::Undo))
            }
            (Outcome::Completed, JobWork::Undo) => match &report.spec {
                JobSpec::Undo(record) => (words::undone(record), None),
                _ => return None,
            },
            (Outcome::Completed, JobWork::Redo) => (words::outcome(Outcome::Completed, work), None),
            (Outcome::Aborted, _) => (words::outcome(Outcome::Aborted, work), None),
            _ => return None,
        };
        let gone = deleted_for_good(report).count();
        Some(self.toast(words::and_gone(text, gone), action))
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

    /// The notice `text` that stays while a worker is at something ([`Effect::Busy`]),
    /// replacing the toast on screen, with its token.
    pub(super) fn busy(&mut self, text: &str) -> (u64, Effect) {
        self.toast_token += 1;
        let token = self.toast_token;
        self.toast = None;
        let text = text.to_owned();
        (token, Effect::Busy { token, text })
    }

    /// A toast with `text`, whose button does `action`, replacing the one on screen.
    pub(super) fn toast(&mut self, text: String, action: Option<ToastAction>) -> Effect {
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
    pub(super) fn open(&mut self, modal: Modal) {
        if self.quitting {
            return;
        }
        // The same confirmation twice (Delete pressed again before the first one showed, or a
        // refusal of items already asked about) asks once.
        if let Modal::Confirm(confirm) = &modal {
            let asks_the_same = |other: &Modal| matches!(other, Modal::Confirm(other) if other.purpose == confirm.purpose);
            if self.modal.iter().chain(&self.later).any(asks_the_same) {
                return;
            }
        }
        if self.modal.is_none() {
            self.modal = Some(modal);
        } else {
            self.later.push_back(modal);
        }
    }

    /// Closes the window that shows; the next one waiting shows instead.
    pub(super) fn close(&mut self) {
        self.modal = self.later.pop_front();
        self.drop_closed_picker();
    }

    /// The items a command acts on in pane `side`: the selected rows, or the cursor's row
    /// when none is selected and the user put the cursor there ([`AppState::target_entries`]).
    fn targets(&self, side: usize) -> Vec<VPath> {
        self.target_entries(side)
            .into_iter()
            .map(|(path, _)| path)
            .collect()
    }

    /// F7, Ctrl+Shift+N: the name window for a new folder in the active tab's folder.
    pub(super) fn new_folder(&mut self, side: usize) -> Vec<Effect> {
        let parent = self.tab(side).location().clone();
        self.open(Modal::Name(NameDialog::new_folder(parent, side)));
        Vec::new()
    }

    /// F2: the name window for the cursor's row, once the user put the cursor there and while
    /// it is not unselected (or else the first selected row).
    pub(super) fn rename(&mut self, side: usize) -> Vec<Effect> {
        let tab = self.tab(side);
        let row = tab
            .selection()
            .user_cursor()
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
    pub(super) fn trash(&mut self, side: usize) -> Vec<Effect> {
        let paths = self.targets(side);
        if paths.is_empty() {
            return Vec::new();
        }
        let folder = self.tab(side).location().clone();
        vec![Effect::CheckTrash { folder, paths }]
    }

    /// Shift+Delete: a confirmation before the items are deleted for good.
    pub(super) fn delete_permanently(&mut self, side: usize) -> Vec<Effect> {
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
    pub(super) fn undo(&mut self) -> Vec<Effect> {
        if self.can_undo() {
            self.toast = None;
            vec![Effect::Job(JobEffect::Undo)]
        } else {
            Vec::new()
        }
    }

    /// Ctrl+Y, Ctrl+Shift+Z: the last undo is done again, when one can be.
    pub(super) fn redo(&mut self) -> Vec<Effect> {
        if self.can_redo() {
            self.toast = None;
            vec![Effect::Job(JobEffect::Redo)]
        } else {
            Vec::new()
        }
    }
}

/// The effect that submits `spec`, asked for in pane `origin`.
pub(super) fn submit(spec: JobSpec, origin: Option<usize>) -> Effect {
    Effect::Job(JobEffect::Submit { spec, origin })
}

/// The warnings of `report` for the items the location deleted for good when they were to go
/// to the trash (`DeletedNotTrashed`): nothing brings them back.
pub(super) fn deleted_for_good(report: &JobReport) -> impl Iterator<Item = &ItemFailure> {
    report
        .warnings
        .iter()
        .filter(|warning| warning.reason == Some(FailureReason::DeletedNotTrashed))
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

/// The paths of the items the job that left `record` worked on.
fn record_paths(record: &UndoRecord) -> Vec<&VPath> {
    match record {
        UndoRecord::Copied { created, .. } => created.iter().map(|item| &item.path).collect(),
        UndoRecord::Moved { moves, .. } => moves.iter().flat_map(|m| [&m.from, &m.to]).collect(),
        UndoRecord::Renamed { from, to } => vec![from, to],
        UndoRecord::Created { path, .. } | UndoRecord::Removed { path, .. } => vec![path],
        UndoRecord::Trashed { items } | UndoRecord::Restored { items } => {
            items.iter().map(|item| &item.original).collect()
        }
        // A record added after this was written.
        _ => Vec::new(),
    }
}

/// The folders a job of `spec` changed, with `outputs`, the paths it made: the folders of its
/// sources and targets, each once, and a transfer's targets themselves, which it merged into
/// where a folder of the same name was there (the tree lists again only the folders it has
/// listed, so a target that is a file, or a folder the job made, costs nothing).
fn touched_folders(spec: &JobSpec, outputs: &[VPath]) -> Vec<VPath> {
    let mut paths: Vec<&VPath> = outputs.iter().collect();
    let mut folders: Vec<VPath> = Vec::new();
    match spec {
        JobSpec::Transfer(transfer) => {
            for item in &transfer.items {
                paths.push(&item.source);
                paths.push(&item.target);
                folders.push(item.target.clone());
            }
        }
        JobSpec::Delete(delete) => paths.extend(&delete.paths),
        JobSpec::Rename { from, to } => paths.extend([from, to]),
        // What an undo took away is in its record; what it put back is in its outputs.
        JobSpec::Undo(record) => paths.extend(record_paths(record)),
        // A create's output is the item it made; a kind of job added after this was written
        // has its outputs.
        _ => {}
    }
    for folder in paths.into_iter().filter_map(VPath::parent) {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders
}
