//! What the window asks of the job engine and what it makes of the engine's news: the file
//! commands, the dialogs over the main window, the job panel, the toasts, the trash check and
//! the app's end. A child of [`state`](super), so these methods see the fields of
//! [`AppState`].
//!
//! XFER-09: a copy or a move runs under the error policy of 보기 ▸ 설정
//! ([`AppState::on_error`]): by default it carries on past an item that fails and its end
//! lists the failed items, whose window offers 다시 시도 ([`AppState::failures_retried`]: a new
//! job of the failed items alone, once for each job); with 파일마다 묻기 the job asks about each
//! failed item ([`AppState::error_answered`]). A job that was stopped (중단) says so in its
//! notice and at the head of its failed items, with how many items it never came to, and
//! offers no retry: the failed items are not all it left undone ([`retry_of`]).

use std::ffi::{OsStr, OsString};
use std::slice;
use std::sync::Arc;
use std::time::Duration;

use rf_core::{EntryKind, VPath};
use rf_jobs::{
    DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobEvent, JobReport,
    JobSpec, LanePolicy, Outcome, TransferMode, TransferOptions, TransferSpec, UndoRecord,
    retry_spec,
};

use super::{AppState, Effect, MOVE_TOAST_TIME, RIGHT, ToastAction};
use crate::dialogs::{
    Confirm, ConfirmPurpose, ConflictChoice, ConflictQuestion, DeleteWhy, ErrorChoice,
    ErrorQuestion, Failures, Modal, NameCheck, NameDialog, NamePurpose, PreflightQuestion,
    Recovery, Retry,
};
use crate::jobs::{Ended, JobEffect, JobRow, Started};
use crate::loader::LoadRequest;
use crate::pane::{LoadStatus, Pane, Tab};
use crate::resolver::{Asked, Question, SKIP_ERROR};
use crate::settings::OnError;
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

    /// What a copy or a move does about an item that fails (보기 ▸ 설정, XFER-09).
    pub fn on_error(&self) -> OnError {
        self.on_error
    }

    /// Sets what a copy or a move does about an item that fails. The jobs submitted from now on
    /// follow it; one that runs keeps the policy it was submitted with.
    pub fn set_on_error(&mut self, on_error: OnError) {
        self.on_error = on_error;
    }

    /// The error policy the transfers this window submits run under ([`AppState::on_error`]).
    pub(super) fn error_policy(&self) -> ErrorPolicy {
        self.on_error.policy()
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

    /// Follows the job the engine started for an [`Effect::Job`]. A move a drop submitted is
    /// noted under its job, for the toast of its end (DND-07).
    pub fn track(&mut self, started: Started) {
        if let Some(spec) = started.spec.as_deref().filter(|_| !started.via_history)
            && let Some(at) = self
                .drag_submits
                .iter()
                .position(|(submitted, _)| submitted == spec)
        {
            let (_, count) = self.drag_submits.remove(at);
            self.dragged_moves.insert(started.id, count);
        }
        let marks = self
            .panes
            .iter()
            .flat_map(Pane::tabs)
            .map(|tab| (tab.id(), tab.selection_epoch()))
            .collect();
        self.selections_at_submit.insert(started.id, marks);
        self.jobs.track(started);
    }

    /// Takes the lanes the engine runs the jobs in ([`Jobs::lanes`](crate::jobs::Jobs::lanes)),
    /// which tell which queued jobs wait behind a paused one.
    pub fn set_lanes(&mut self, lanes: Arc<dyn LanePolicy>) {
        self.jobs.set_lanes(lanes);
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
                Some(ended) => self.finished(ended),
                None => Vec::new(),
            },
        }
    }

    /// Takes a question of a job: it shows once the windows before it have closed. While the
    /// app ends, it is left unanswered, for the release that follows the engine's shutdown.
    pub fn question(&mut self, question: Question) -> Vec<Effect> {
        // While the app removes itself, a question that comes before the engine ends is kept
        // aside with the windows that were open ([`AppState::removal_stopped`]).
        if self.quitting && !self.removing {
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
            Asked::Error(failure) => Modal::Error(ErrorQuestion {
                id,
                job,
                failure,
                items_left: self.jobs.items_left(job),
                work: self.jobs.work(job).unwrap_or(JobWork::Other),
            }),
            Asked::Preflight(issues) => Modal::Preflight(PreflightQuestion { id, job, issues }),
        };
        if self.removing {
            self.removal_stash.push(modal);
            return Vec::new();
        }
        self.open(modal);
        // Behind the tray icon, nobody would see it: the window comes back to ask.
        self.follow_background()
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
        // A favorite's name is a label the state keeps: no job renames anything.
        if let NamePurpose::Favorite { index } = dialog.purpose {
            return match dialog.check() {
                NameCheck::Ready => {
                    let name = dialog.text.clone();
                    self.close();
                    self.favorite_rename(index, &name)
                }
                NameCheck::Unchanged => {
                    self.close();
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }
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
    /// button): a job question is answered with its cancelling choice, which stops the job;
    /// the question about an item that failed with the skip of that item, after which the job
    /// goes on (XFER-09).
    pub fn dialog_cancelled(&mut self) -> Vec<Effect> {
        let effects = match &self.modal {
            Some(Modal::Conflict(question)) => vec![Effect::Job(JobEffect::AnswerConflict(
                question.id,
                rf_jobs::ConflictAnswer::once(rf_jobs::ConflictAction::Cancel),
            ))],
            Some(Modal::Error(question)) => {
                vec![Effect::Job(JobEffect::AnswerError(question.id, SKIP_ERROR))]
            }
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
    /// delete of the copies an undo could not trash looks at each copy and its original once
    /// more itself before it deletes it.
    pub fn confirm_accepted(&mut self) -> Vec<Effect> {
        let Some(Modal::Confirm(confirm)) = &self.modal else {
            return Vec::new();
        };
        match &confirm.purpose {
            ConfirmPurpose::Quit => self.begin_quit(),
            ConfirmPurpose::DeleteCopies { spec, record } => {
                let (spec, record) = (spec.clone(), UndoRecord::clone(record));
                self.close();
                self.copies_confirmed(spec, record)
            }
            ConfirmPurpose::DeletePermanently(_) => {
                let spec = confirm.job();
                self.close();
                spec.map(|spec| submit(spec, None)).into_iter().collect()
            }
            ConfirmPurpose::ApplyViewBelow { folder, view } => {
                let (folder, view) = (folder.clone(), view.clone());
                self.close();
                self.apply_view_below(&folder, view)
            }
            ConfirmPurpose::RemoveApp => {
                let backup = confirm.checked;
                self.removal_confirmed(backup)
            }
            ConfirmPurpose::RemoveAppWithJobs { backup } => {
                let backup = *backup;
                self.begin_removal(backup)
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

    /// The error window was answered with `choice`, for the later failures of the job too when
    /// `apply_all` (남은 오류에도 같은 선택). A retry of every later failure is not offered, and
    /// does nothing.
    pub fn error_answered(&mut self, choice: ErrorChoice, apply_all: bool) -> Vec<Effect> {
        let Some(Modal::Error(question)) = &self.modal else {
            return Vec::new();
        };
        let Some(answer) = ErrorQuestion::answer(choice, apply_all) else {
            return Vec::new();
        };
        let id = question.id;
        self.close();
        vec![Effect::Job(JobEffect::AnswerError(id, answer))]
    }

    /// 다시 시도 of the window of failed items (XFER-09): the job of the failed items alone is
    /// submitted ([`Retry`]), the window closes, and the job they come from offers them no
    /// second time (its retry tells of its own failures). Nothing for a window without the
    /// button.
    pub fn failures_retried(&mut self) -> Vec<Effect> {
        let Some(Modal::Failures(failures)) = &self.modal else {
            return Vec::new();
        };
        let Some(Retry { job, spec }) = failures.retry.clone() else {
            return Vec::new();
        };
        self.close();
        // Once for each job, whichever window of it the press came from.
        if !self.retried_jobs.insert(job) {
            return Vec::new();
        }
        vec![submit(spec, None)]
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
            ToastAction::Details(mut failures) => {
                // Its failed items may have been tried again from 최근 완료 since the toast
                // showed: they are not offered a second time.
                let retried = |retry: &Retry| self.retried_jobs.contains(&retry.job);
                if failures.retry.as_ref().is_some_and(retried) {
                    failures.retry = None;
                }
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
    /// that shows, which comes back if the user stays), which may offer to go on behind the
    /// tray icon (module `tray`); without, the app ends.
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
        // The jobs may go on behind the tray icon, where it is offered and nothing else waits
        // for the user.
        let tray = self.tray_offer();
        if let Some(shown) = self.modal.take() {
            self.later.push_front(shown);
        }
        self.modal = Some(Modal::Confirm(Confirm::quit(running, tray)));
        Vec::new()
    }

    /// The app ends: every window closes, no new one opens, and the questions still open stay
    /// unanswered here. [`Effect::Quit`] then shuts the engine down before it releases them
    /// (carry-m1-03 "quit order").
    pub fn begin_quit(&mut self) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        self.close_for_end();
        vec![Effect::Quit]
    }

    /// Every window closes and nothing opens or asks any more: the app ends, or the removal of
    /// the app works ([`AppState::begin_removal`]).
    pub(super) fn close_for_end(&mut self) {
        self.quitting = true;
        self.modal = None;
        self.later.clear();
        self.drop_closed_picker();
        self.drop_closed_pattern();
        self.toast = None;
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
    pub(super) fn job_ended(&mut self, ended: Ended) -> Vec<Effect> {
        let Ended {
            work,
            origin,
            report,
            ..
        } = ended;
        // A move a drop made, and how many items it carried.
        let dragged = self.dragged_moves.remove(&report.id);
        let touched = touched_folders(&report.spec, &report.outputs);
        let mut effects = self.folders_changed(&touched);
        // A move a paste submitted empties the clipboard once it has moved everything.
        effects.extend(self.paste_ended(&report));
        let pasted_in = self.pasted_in(&report);
        effects.extend(self.pick_made(&report));
        // Whatever it did, a cancelled job too, it may have changed the room left.
        if !self.quitting {
            effects.push(self.space_to_read(&touched));
        }
        // What the selection was when the job was submitted.
        let marks = self.selections_at_submit.remove(&report.id);
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
            effects.extend(self.select_outputs(side, slice::from_ref(output), marks.as_deref()));
        }
        effects.extend(self.select_made_links(work, origin, &report, marks.as_deref()));
        // What a paste made is selected in the pane it was asked in, as File Explorer does;
        // what it could not make (a failure) is not among its outputs.
        if matches!(
            report.outcome,
            Outcome::Completed | Outcome::CompletedWithErrors
        ) && let Some((side, mark)) = pasted_in
        {
            // Only while the selection is as it was when the paste was asked, before its read.
            effects.extend(self.select_outputs(side, &report.outputs, Some(&[mark])));
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
                effects.extend(self.select_outputs(
                    side,
                    slice::from_ref(output),
                    marks.as_deref(),
                ));
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
        // A move a drop made that moved everything offers its undo for a moment (DND-07); one
        // that was cut short, or copied for want of rights on the source, is told as others are.
        let moved_by_drag = dragged
            .filter(|_| report.outcome == Outcome::Completed && !report.copied_instead_of_moved);
        if !failures.is_empty() {
            effects.push(self.failure_notice(work, &failures, &report));
        } else if work == JobWork::Trash {
            effects.extend(self.trash_toast(&report));
        } else if let Some(count) = moved_by_drag {
            effects.extend(self.dragged_toast(count, &report));
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
    /// items of `report` the location deleted for good instead of trashing them. A job that was
    /// stopped says that instead, in the toast and as the first line of the list
    /// ([`stopped_note`]).
    fn failure_notice(
        &mut self,
        work: JobWork,
        failures: &[&ItemFailure],
        report: &JobReport,
    ) -> Effect {
        if matches!(work, JobWork::Create | JobWork::Rename) {
            return Effect::Notice(words::name_refused(work, failures[0]));
        }
        // One link that could not be made says why in the notice itself.
        if work == JobWork::Link && failures.len() == 1 {
            return Effect::Notice(words::link_refused(failures[0]));
        }
        let gone: Vec<&ItemFailure> = deleted_for_good(report).collect();
        let listed = failures.iter().chain(&gone);
        let shown = Failures {
            title: words::failures_title().to_owned(),
            lines: stopped_note(work, report)
                .into_iter()
                .chain(listed.map(|failure| words::failure_line(failure)))
                .collect(),
            retry: retry_of(report),
        };
        let text = if report.outcome == Outcome::Aborted {
            words::job_stopped(work, failures.len(), not_reached(report))
        } else {
            words::items_failed(failures.len())
        };
        self.toast(
            words::and_gone(text, gone.len()),
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
            (Outcome::Completed, JobWork::Link) => {
                let dangling = report
                    .warnings
                    .iter()
                    .filter(|warning| warning.reason == Some(FailureReason::LinkTargetMissing))
                    .count();
                let text = words::links_made(report.done.files, dangling);
                (text, report.undo.clone().map(ToastAction::Undo))
            }
            (Outcome::Completed, JobWork::Redo) => (words::outcome(Outcome::Completed, work), None),
            (Outcome::Aborted, _) => (words::outcome(Outcome::Aborted, work), None),
            _ => return None,
        };
        let gone = deleted_for_good(report).count();
        Some(self.toast(words::and_gone(text, gone), action))
    }

    /// The toast of a move of `count` items that a drop made and that moved them without
    /// failures (DND-07): what it moved, for [`MOVE_TOAST_TIME`], with a button that undoes it
    /// (as the history's record, whose undo checks for later changes). None while 보기 ▸ 설정
    /// has it off: Ctrl+Z undoes the move then.
    fn dragged_toast(&mut self, count: usize, report: &JobReport) -> Option<Effect> {
        if !self.move_undo_toast {
            return None;
        }
        let done = report.done.files.saturating_add(report.done.dirs);
        let text = words::dragged_move_done(count, done, report.skipped);
        let action = report.undo.clone().map(ToastAction::Undo);
        let gone = deleted_for_good(report).count();
        Some(self.toast_for(words::and_gone(text, gone), action, Some(MOVE_TOAST_TIME)))
    }

    /// What a link job made is selected in the pane it was asked in, so that the new links are
    /// what the next key acts on (OPS-08); see [`AppState::select_outputs`].
    fn select_made_links(
        &mut self,
        work: JobWork,
        origin: Option<usize>,
        report: &JobReport,
        marks: Option<&[(u64, u64)]>,
    ) -> Vec<Effect> {
        match origin {
            Some(side)
                if work == JobWork::Link
                    && matches!(
                        report.outcome,
                        Outcome::Completed | Outcome::CompletedWithErrors
                    ) =>
            {
                self.select_outputs(side, &report.outputs, marks)
            }
            _ => Vec::new(),
        }
    }

    /// Selects `outputs`, the items a create, a rename, an undo or a paste made, in pane `side`:
    /// those in the folder it still shows, now if their rows are there, else as they come. Not
    /// when the user has changed the selection of that tab since the job was submitted, as
    /// `marks` (the tabs' selection epochs then, if they were noted) tell, or when the tab was
    /// not there then: what the user chose stays, so that a key pressed right then (Delete) acts
    /// on it.
    fn select_outputs(
        &mut self,
        side: usize,
        outputs: &[VPath],
        marks: Option<&[(u64, u64)]>,
    ) -> Vec<Effect> {
        let tab = self.tab_mut(side);
        let names: Vec<OsString> = outputs
            .iter()
            .filter(|output| output.parent().as_ref() == Some(tab.location()))
            .filter_map(|output| output.file_name().map(OsStr::to_os_string))
            .collect();
        if names.is_empty() {
            return Vec::new();
        }
        let untouched =
            marks.is_none_or(|marks| marks.contains(&(tab.id(), tab.selection_epoch())));
        if !untouched {
            return Vec::new();
        }
        tab.select_names(&names)
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
        self.toast_for(text, action, None)
    }

    /// [`AppState::toast`] for `time`, or for the window's usual time (`None`).
    pub(super) fn toast_for(
        &mut self,
        text: String,
        action: Option<ToastAction>,
        time: Option<Duration>,
    ) -> Effect {
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
            time,
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
        self.drop_closed_pattern();
    }

    /// The items a command acts on in pane `side`: the selected rows, or the cursor's row
    /// when none is selected and the user put the cursor there ([`AppState::target_entries`]).
    pub(super) fn targets(&self, side: usize) -> Vec<VPath> {
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

/// What 다시 시도 of the window of the failed items of `report` submits (XFER-09): for a copy or
/// a move, the transfer of the failed items alone, with the mode, the targets and the policies
/// of the job they come from (`rf_jobs::retry_spec`). `None` for another kind of job, and for a
/// transfer none of whose failures can be tried again (a source that could not be removed
/// after its copy was in place, a link that leads back to its own folder).
///
/// `None` too for a job that was stopped (`Outcome::Aborted`): the items it never came to are
/// no failures, and the report does not name them (it counts them at most), so a retry would
/// redo the failed item alone and look as if it had finished the job. The window says that
/// the job was stopped instead ([`stopped_note`]).
pub(super) fn retry_of(report: &JobReport) -> Option<Retry> {
    if !matches!(report.spec, JobSpec::Transfer(_)) || report.outcome == Outcome::Aborted {
        return None;
    }
    let spec = retry_spec(&report.spec, &report.failures)?;
    Some(Retry {
        job: report.id,
        spec,
    })
}

/// How many items a job that was stopped never came to, as far as `report` tells: what it
/// planned, less what it did, skipped and failed at. `None` when it cannot be told (the job
/// stopped while it was still counting), `Some(0)` when the item it stopped at was its last.
pub(super) fn not_reached(report: &JobReport) -> Option<u64> {
    let items = |totals: rf_jobs::Totals| totals.files.saturating_add(totals.dirs);
    let planned = items(report.planned);
    if planned == 0 {
        return None;
    }
    let failed = u64::try_from(report.failures.len()).unwrap_or(u64::MAX);
    Some(
        planned
            .saturating_sub(items(report.done))
            .saturating_sub(report.skipped)
            .saturating_sub(failed),
    )
}

/// The line that heads the failed items of a job that was stopped (`Outcome::Aborted`: 중단 in
/// the window that asks about a failed item, or an error that stopped the job): that it was
/// stopped, and that what came after the listed items was not processed. `None` for a job
/// that ran to its end.
pub(super) fn stopped_note(work: JobWork, report: &JobReport) -> Option<String> {
    (report.outcome == Outcome::Aborted).then(|| words::job_stopped_note(work, not_reached(report)))
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
pub(super) fn top_level_count(spec: &JobSpec) -> usize {
    match spec {
        JobSpec::Transfer(transfer) => transfer.items.len(),
        JobSpec::Delete(delete) => delete.paths.len(),
        JobSpec::DeleteCopies(delete) => delete.copies.len(),
        JobSpec::MakeLink { links } => links.len(),
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
        UndoRecord::LinksMade { at: links } | UndoRecord::LinksRemoved { links } => {
            links.iter().map(|(link, _, _)| link).collect()
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
        JobSpec::DeleteCopies(delete) => paths.extend(delete.copies.iter().map(|c| &c.made.path)),
        JobSpec::Rename { from, to } => paths.extend([from, to]),
        JobSpec::MakeLink { links } => paths.extend(links.iter().map(|(_, at, _)| at)),
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
