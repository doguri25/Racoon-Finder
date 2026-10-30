//! The jobs going on behind the tray icon while the window is hidden (XFER-15). A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! Where the system has a tray ([`rf_platform::desktop::TRAY`]: the notification area on
//! Windows, the menu bar on macOS), the question before the window closes while jobs run also
//! offers 트레이에서 계속 (메뉴 막대에서 계속), as its main answer, when nothing else waits for
//! the user. Then the window hides ([`Effect::ToTray`]) and the tray icon shows how far the jobs
//! have got ([`AppState::tray_view`]), with a menu of 창 열기, 모두 일시정지 (or 모두 계속) and
//! 끝내기 ([`TrayCommand`]). The settings are saved as before meanwhile.
//!
//! Whatever asks the user brings the window back ([`Effect::FromTray`]): a question of a job,
//! or a window a job's end opens. Once the last job has ended, a notification says so and the
//! app ends the way it always does ([`AppState::begin_quit`]: the settings are written, the
//! engine shuts down, then the questions are released). 끝내기 while jobs run brings the window
//! back with the question before the app ends, without the tray this time.

use rf_core::Os;
use rf_jobs::{JobId, JobReport, JobState};

use super::jobs::top_level_count;
use super::{AppState, Effect};
use crate::dialogs::{Confirm, ConfirmPurpose, Modal};
use crate::jobs::JobEffect;
use crate::notify::{Notice, TrayView};
use crate::words::{self, JobWork};

/// An item of the tray icon's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrayCommand {
    /// 창 열기 (and a click on the icon): the window shows again.
    Open,
    /// 모두 일시정지.
    PauseAll,
    /// 모두 계속.
    ResumeAll,
    /// 끝내기.
    Quit,
}

impl AppState {
    /// Takes whether the system has a tray the jobs can go on behind.
    pub fn set_tray(&mut self, supported: bool) {
        self.tray = supported;
    }

    /// Whether the window is hidden and the jobs go on behind the tray icon.
    pub fn background(&self) -> bool {
        self.background
    }

    /// What the tray icon shows, while the jobs go on behind it: its tooltip names `app` and
    /// how far each job has got. `None` while the window shows, and as the app ends.
    pub fn tray_view(&self, app: &str) -> Option<TrayView> {
        if !self.background || self.quitting {
            return None;
        }
        let rows = self.jobs.rows();
        let states = self.jobs.states();
        Some(TrayView {
            status: words::tray_status(app, &rows),
            label: rows
                .first()
                .map(|row| words::percent(row.fraction))
                .unwrap_or_default(),
            all_paused: !states.is_empty()
                && states.iter().all(|(_, state)| *state == JobState::Paused),
        })
    }

    /// The third answer of the question that shows, 트레이에서 계속: the window hides, and the
    /// jobs go on behind the tray icon. If the jobs have ended meanwhile, the app ends; if
    /// something else came to ask the user meanwhile, it shows and the window stays.
    pub fn confirm_alternative(&mut self) -> Vec<Effect> {
        let Some(Modal::Confirm(confirm)) = &self.modal else {
            return Vec::new();
        };
        if confirm.purpose != ConfirmPurpose::Quit || confirm.alt.is_empty() {
            return Vec::new();
        }
        self.close();
        if self.jobs.active() == 0 {
            return self.begin_quit();
        }
        if self.modal.is_some() {
            return Vec::new();
        }
        self.background = true;
        vec![Effect::ToTray]
    }

    /// An item of the tray icon's menu.
    pub fn tray_command(&mut self, command: TrayCommand) -> Vec<Effect> {
        if !self.background || self.quitting {
            return Vec::new();
        }
        let states = self.jobs.states();
        let control = |paused: bool, effect: fn(JobId) -> JobEffect| -> Vec<Effect> {
            states
                .iter()
                .filter(|(_, state)| (*state == JobState::Paused) == paused)
                .map(|(id, _)| Effect::Job(effect(*id)))
                .collect()
        };
        match command {
            TrayCommand::PauseAll => control(false, JobEffect::Pause),
            TrayCommand::ResumeAll => control(true, JobEffect::Resume),
            TrayCommand::Open => {
                self.background = false;
                vec![Effect::FromTray]
            }
            TrayCommand::Quit => {
                let running = self.jobs.active();
                if running == 0 {
                    return self.begin_quit();
                }
                self.background = false;
                self.open(Modal::Confirm(Confirm::quit(running, None)));
                vec![Effect::FromTray]
            }
        }
    }

    /// The system the question before the app ends offers the tray on: this one, where it has a
    /// tray, while the window shows and nothing else waits for the user.
    pub(super) fn tray_offer(&self) -> Option<Os> {
        (self.tray && !self.background && self.modal.is_none() && self.later.is_empty())
            .then_some(Os::CURRENT)
    }

    /// The window comes back from behind the tray icon, if it was there, to show what asks the
    /// user.
    pub(super) fn come_back(&mut self) -> Vec<Effect> {
        if self.background && self.modal.is_some() {
            self.background = false;
            return vec![Effect::FromTray];
        }
        Vec::new()
    }

    /// After a job ended as `work` with `report`, of which `notice` tells, if anything: while
    /// the window shows, only the notice. Behind the tray icon, the window comes back if the
    /// job's end asks the user something; and once the last job has ended, one notification
    /// tells of it, whatever time it took, and the app ends.
    pub(super) fn after_job(
        &mut self,
        work: JobWork,
        report: &JobReport,
        notice: Option<Notice>,
    ) -> Vec<Effect> {
        let mut effects: Vec<Effect> = Vec::new();
        if !self.background || self.quitting {
            effects.extend(notice.map(Effect::Notify));
            return effects;
        }
        if self.modal.is_some() || self.jobs.active() > 0 {
            effects.extend(notice.map(Effect::Notify));
            effects.extend(self.come_back());
            return effects;
        }
        let title = words::job_notice(
            work,
            report.outcome,
            top_level_count(&report.spec),
            report.failures.len(),
        )
        .unwrap_or_else(|| words::outcome(report.outcome, work));
        let detail = words::job_detail(&report.spec);
        let body = if detail.is_empty() {
            words::all_jobs_done().to_owned()
        } else {
            format!("{detail}\n{}", words::all_jobs_done())
        };
        effects.push(Effect::Notify(Notice { title, body }));
        effects.extend(self.begin_quit());
        effects
    }
}
