//! What the window keeps and tells of the jobs that ended (XFER-14): the job panel's 최근 완료,
//! the log of one of them saved where the user chooses with the destination picker (PICK-09),
//! and the system's notification of a job that ended while the window was not in front. A
//! child of [`state`](super), so these methods see the fields of [`AppState`].
//!
//! Every job that ends goes first into 최근 완료 ([`RECENT_JOBS`] at most, the newest first),
//! but for a rename and a new folder, which the list shows at once by the item itself, and the
//! jobs that end as the app does. A job that took longer than [`NOTIFY_AFTER`] and ended while
//! the window was not in front (or hidden behind the tray icon) is told of by a notification
//! too, unless the user cancelled it.
//!
//! [`RECENT_JOBS`]: crate::jobs::RECENT_JOBS

use std::sync::Arc;

use rf_core::{ErrorKind, VPath};
use rf_jobs::JobReport;

use super::jobs::top_level_count;
use super::{AppState, Effect};
use crate::dialogs::{Failures, Modal};
use crate::jobs::{Ended, RecentRow, report_lines};
use crate::notify::{NOTIFY_AFTER, Notice};
use crate::pick::{PickPurpose, Picker, log_request, log_targets};
use crate::words::{self, JobWork};

impl AppState {
    /// The lines of 최근 완료, the newest first.
    pub fn recent_rows(&self) -> Vec<RecentRow> {
        self.recent_jobs.rows()
    }

    /// Whether 최근 완료 shows its lines below its head.
    pub fn recent_open(&self) -> bool {
        self.recent_open
    }

    /// Takes whether the window is the one in front, as a job ends.
    pub fn set_window_active(&mut self, active: bool) {
        self.window_active = active;
    }

    /// Takes the app's folder for logs, where 로그 저장… offers to put a job's log first.
    pub fn set_log_dir(&mut self, dir: VPath) {
        self.log_dir = Some(dir);
    }

    /// 최근 완료's 펼치기 or 접기.
    pub fn recent_toggled(&mut self) {
        self.recent_open = !self.recent_open;
    }

    /// 최근 완료's 지우기: the list empties, and the job panel goes with it unless a job runs.
    pub fn recent_cleared(&mut self) {
        self.recent_jobs.clear();
        self.recent_open = false;
    }

    /// 자세히 of line `index` of 최근 완료: the items the job could not process, and why, and those
    /// it warned about. Nothing for a job without them.
    pub fn recent_details(&mut self, index: usize) -> Vec<Effect> {
        let Some(finished) = self.recent_jobs.get(index) else {
            return Vec::new();
        };
        let lines = report_lines(&finished.report.0);
        if lines.is_empty() || self.quitting {
            return Vec::new();
        }
        self.open(Modal::Failures(Failures {
            title: words::failures_title().to_owned(),
            lines,
        }));
        Vec::new()
    }

    /// 로그 저장… of line `index` of 최근 완료: the destination picker asks where the log goes,
    /// the app's folder for logs chosen first (PICK-09).
    pub fn save_log_asked(&mut self, index: usize) -> Vec<Effect> {
        let (Some(finished), Some(log_dir)) =
            (self.recent_jobs.get(index).cloned(), self.log_dir.clone())
        else {
            return Vec::new();
        };
        if self.quitting {
            return Vec::new();
        }
        let row = &self.recent_jobs.rows()[index];
        let purpose = PickPurpose::SaveLog {
            report: finished.report,
            title: row.title.clone(),
            detail: row.detail.clone(),
        };
        let request = log_request(log_dir.clone());
        let groups = log_targets(
            &request,
            &log_dir,
            &self.open_tab_folders(),
            &self.recent,
            &self.places,
        );
        let picker = Picker::for_log(purpose, request, self.active, groups);
        self.show_picker(picker)
    }

    /// What came of a log written by a worker ([`Effect::SaveLog`]): the name it was written
    /// under, or why it was not.
    pub fn log_saved(&mut self, saved: Result<String, ErrorKind>) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        let text = match saved {
            Ok(name) => words::log_saved(&name),
            Err(kind) => words::log_failed(kind),
        };
        vec![self.toast(text, None)]
    }

    /// A job ended: it goes into 최근 완료, it is told of as the window would have it
    /// ([`AppState::job_ended`]), and, while the window is hidden, the app ends once the last
    /// one has (module `tray`).
    pub(super) fn finished(&mut self, ended: Ended) -> Vec<Effect> {
        let work = ended.work;
        let report = Arc::clone(&ended.report);
        if !self.quitting && !matches!(work, JobWork::Create | JobWork::Rename) {
            self.recent_jobs.add(work, Arc::clone(&report));
        }
        let notice = self.job_notice(work, &report);
        let mut effects = self.job_ended(ended);
        effects.extend(self.after_job(work, &report, notice));
        effects
    }

    /// The notification of the job that ended as `work` with `report`: for a job that took
    /// longer than [`NOTIFY_AFTER`], when the window was not in front or was hidden, and the
    /// user did not cancel it.
    fn job_notice(&self, work: JobWork, report: &JobReport) -> Option<Notice> {
        let unseen = !self.window_active || self.background;
        if self.quitting || !unseen || report.elapsed <= NOTIFY_AFTER {
            return None;
        }
        Some(Notice {
            title: words::job_notice(
                work,
                report.outcome,
                top_level_count(&report.spec),
                report.failures.len(),
            )?,
            body: words::job_detail(&report.spec),
        })
    }
}
