//! The job panel from the keyboard (KEY-02). A child of [`state`](super), so these methods see
//! the fields of [`AppState`].
//!
//! F6 goes round the parts of the window, as File Explorer's does: from the panes to the
//! sidebar's tree ([`AppState::tree_key`]), from there to the job panel while it shows, and
//! from the panel back to the panes. The panel's buttons are in lines: each job's row has
//! 일시정지 (or 계속) and 취소; the head of 최근 완료 has 펼치기 (or 접기) and 지우기; and while
//! that list is open, each finished job's line has 자세히, when it has items to tell of, and 로그
//! 저장…, which is also the way of the keys to the 자세히 of a toast: the toast's job is in the
//! list. The keys of [`PanelKey`] move among them, as among a dialog's
//! buttons, and press the one the keys are on; Esc, Tab and F6 give the keys back to the active
//! pane. While the panel has the keys, the window's other commands work as from the panes, and
//! those of the panes' rows do nothing
//! ([`Command::acts_on_rows`](crate::keys::Command::acts_on_rows)), as in the tree.

use super::{AppState, Effect};
use crate::jobs::Finished;
use crate::keys::PanelKey;

/// A button of the job panel, as the keys reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelButton {
    /// 일시정지, or 계속 while it is paused, of the job of row `row`.
    PauseOrResume(usize),
    /// 취소 of the job of row `row`.
    Cancel(usize),
    /// 펼치기 or 접기 of 최근 완료.
    ToggleRecent,
    /// 지우기 of 최근 완료.
    ClearRecent,
    /// 자세히 of line `index` of 최근 완료.
    Details(usize),
    /// 로그 저장… of line `index` of 최근 완료.
    SaveLog(usize),
}

impl AppState {
    /// The buttons of the job panel, a line at a time, in the order they show: one line for
    /// each job's row, then the head of 최근 완료, then, while it is open, each of its lines.
    /// Empty while the panel does not show.
    pub fn panel_lines(&self) -> Vec<Vec<PanelButton>> {
        let mut lines: Vec<Vec<PanelButton>> = (0..self.jobs.rows().len())
            .map(|row| vec![PanelButton::PauseOrResume(row), PanelButton::Cancel(row)])
            .collect();
        if self.recent_jobs.is_empty() {
            return lines;
        }
        lines.push(vec![PanelButton::ToggleRecent, PanelButton::ClearRecent]);
        if self.recent_open {
            for index in 0..self.recent_jobs.len() {
                let mut line = Vec::new();
                if self
                    .recent_jobs
                    .get(index)
                    .is_some_and(Finished::has_details)
                {
                    line.push(PanelButton::Details(index));
                }
                line.push(PanelButton::SaveLog(index));
                lines.push(line);
            }
        }
        lines
    }

    /// The line and the button of the panel's keyboard cursor, kept within the lines there are
    /// now; `None` while the panel does not show.
    pub fn panel_cursor(&self) -> Option<(usize, usize)> {
        let lines = self.panel_lines();
        let (line, button) = self.panel_at;
        let line = line.min(lines.len().checked_sub(1)?);
        let button = button.min(lines[line].len().checked_sub(1)?);
        Some((line, button))
    }

    /// F6 from the tree: the job panel takes the keys while it shows, on its first button;
    /// otherwise the panes do.
    pub(super) fn tree_next(&mut self) -> Vec<Effect> {
        if self.panel_lines().is_empty() {
            return vec![Effect::FocusPanes];
        }
        self.panel_at = (0, 0);
        vec![Effect::FocusPanel]
    }

    /// A key of the job panel while it has the keys ([`crate::keys::panel_key`]). A panel
    /// with nothing left in it gives the keys back.
    pub fn panel_key(&mut self, key: PanelKey) -> Vec<Effect> {
        let lines = self.panel_lines();
        let Some((line, button)) = self.panel_cursor() else {
            return vec![Effect::FocusPanes];
        };
        let to = match key {
            PanelKey::Leave => return vec![Effect::FocusPanes],
            PanelKey::Press => return self.press_panel(lines[line][button]),
            PanelKey::Up => (line.saturating_sub(1), button),
            PanelKey::Down => ((line + 1).min(lines.len() - 1), button),
            PanelKey::Left => (line, button.saturating_sub(1)),
            PanelKey::Right => (line, (button + 1).min(lines[line].len() - 1)),
        };
        // Up and down keep the column where the line has one, else take its last button.
        self.panel_at = (to.0, to.1.min(lines[to.0].len() - 1));
        Vec::new()
    }

    /// What `button` of the panel does when pressed: what a click on it does.
    fn press_panel(&mut self, button: PanelButton) -> Vec<Effect> {
        match button {
            PanelButton::PauseOrResume(row) => {
                let paused = self.jobs.rows().get(row).is_some_and(|row| row.paused);
                self.job_control(row, i32::from(paused))
            }
            PanelButton::Cancel(row) => self.job_control(row, 2),
            PanelButton::ToggleRecent => {
                self.recent_toggled();
                Vec::new()
            }
            PanelButton::ClearRecent => {
                self.recent_cleared();
                Vec::new()
            }
            PanelButton::Details(index) => self.recent_details(index),
            PanelButton::SaveLog(index) => self.save_log_asked(index),
        }
    }
}
