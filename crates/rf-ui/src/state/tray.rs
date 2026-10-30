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
//! The window is never left hidden with nothing to show it again: it comes back
//! ([`Effect::FromTray`]) whenever something waits for the user (a question of a job, a window
//! a job's end opens, a look at copies that may end in one, [`AppState::follow_background`]),
//! when every job left is paused or waits behind a paused one (nothing would end them), and
//! when the last job has ended but one that ended out of sight did not do it all, so that 최근
//! 완료 and its 자세히 show. Only when the last job has ended with everything done does a
//! notification say so and the app end the way it always does ([`AppState::begin_quit`]: the
//! settings are written, the engine shuts down, then the questions are released). Nor is the
//! tray offered while every job is paused or waits behind a paused one, or while something
//! waits for the user. 끝내기 while jobs run brings the window back with the question before the
//! app ends, without the tray this time. A second start of the app shows the window too
//! ([`AppState::show_asked`], module `instance`), and so does a tray icon the window could not
//! make ([`AppState::tray_unavailable`]), which the window then never hides behind.

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
        if self.waits_for_user() {
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
            TrayCommand::Open => self.come_back(),
            TrayCommand::Quit => {
                let running = self.jobs.active();
                if running == 0 {
                    return self.begin_quit();
                }
                self.open(Modal::Confirm(Confirm::quit(running, None)));
                self.come_back()
            }
        }
    }

    /// The system the question before the app ends offers the tray on: this one, where it has a
    /// tray, while the window shows, nothing else waits for the user (a window, or a look at
    /// copies that may end in one, which would bring the window straight back), and a job goes
    /// on (not when every job is paused, or waits behind a paused one, which nothing behind the
    /// tray would end).
    pub(super) fn tray_offer(&self) -> Option<Os> {
        (self.tray && !self.background && !self.waits_for_user() && self.jobs_go_on())
            .then_some(Os::CURRENT)
    }

    /// Whether a job is under way that will end by itself: one that is not paused, and not
    /// queued in a lane whose job under way is paused ([`JobRows::going_on`]).
    ///
    /// [`JobRows::going_on`]: crate::jobs::JobRows::going_on
    fn jobs_go_on(&self) -> bool {
        self.jobs.going_on()
    }

    /// Whether something waits for the user: a window over the main window, or one waiting to
    /// show, or a look at copies an undo could not trash, which may end in a question.
    fn waits_for_user(&self) -> bool {
        self.modal.is_some() || !self.later.is_empty() || self.checking_copies()
    }

    /// The window comes back from behind the tray icon, if it is there, when something waits
    /// for the user: called after every change of the state, so that no question and no
    /// confirmation is ever left behind a hidden window.
    pub fn follow_background(&mut self) -> Vec<Effect> {
        if self.background && self.waits_for_user() {
            return self.come_back();
        }
        Vec::new()
    }

    /// A second start of the app asked this one to show its window (module `instance`): it
    /// comes back from behind the tray icon, or to the front.
    pub fn show_asked(&mut self) -> Vec<Effect> {
        self.come_back()
    }

    /// The window could not make the tray icon: it stays shown, and the tray is not offered
    /// any more.
    pub fn tray_unavailable(&mut self) -> Vec<Effect> {
        self.tray = false;
        self.background = false;
        self.background_failed = false;
        vec![Effect::Notice(words::tray_unavailable().to_owned())]
    }

    /// The window shows again, from behind the tray icon or to the front.
    fn come_back(&mut self) -> Vec<Effect> {
        self.background = false;
        self.background_failed = false;
        vec![Effect::FromTray]
    }

    /// After a job ended as `work` with `report`, of which `notice` tells, if anything: while
    /// the window shows, only the notice. Behind the tray icon the jobs go on hidden while one
    /// that is not paused is left and nothing waits for the user; otherwise the window comes
    /// back. Once the last job has ended, one notification tells of it, whatever time it took,
    /// and the app ends, unless a job that ended out of sight left items it could not process:
    /// then the window comes back to show them.
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
        if !report.failures.is_empty() {
            self.background_failed = true;
        }
        let ended = self.jobs.active() == 0;
        if !ended || self.waits_for_user() {
            effects.extend(notice.map(Effect::Notify));
            if self.waits_for_user() || !self.jobs_go_on() {
                effects.extend(self.come_back());
            }
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
        let last = if self.background_failed {
            words::all_jobs_done_with_failures()
        } else {
            words::all_jobs_done()
        };
        let body = if detail.is_empty() {
            last.to_owned()
        } else {
            format!("{detail}\n{last}")
        };
        effects.push(Effect::Notify(Notice { title, body }));
        if self.background_failed {
            effects.extend(self.come_back());
        } else {
            effects.extend(self.begin_quit());
        }
        effects
    }
}
