//! The selection tools of the panes (NAV-15), listing a folder again (NAV-13) and showing the
//! extensions of the names or not (NAV-11).
//!
//! Every tool chooses among the rows on screen: the items the hidden-items setting and the
//! filter box show, in any view mode (a hidden or filtered-out item is never selected, as in
//! File Explorer). Selecting all, none, the inverse and the rows with the extension of the
//! cursor's row happen at once on the UI thread. The pattern window (패턴으로 선택) has a worker
//! match the names ([`Effect::MatchNames`]) while the pattern is typed, so that a folder of
//! 100,000 items does not stop the window at each key: each request has a new generation, the
//! worker drops a request as soon as a newer one comes, and the window takes only the answer
//! for the pattern as it is ([`AppState::pattern_matched`]). The rows its 선택 selects are those
//! the worker matched, so no pattern is matched on the UI thread (unless the worker failed);
//! when the rows change while it shows (a watch), the worker matches them again, and a 선택 that
//! waited goes on waiting. A window that waits behind the question whether to quit keeps taking
//! the answers and the changes of the rows, so it is up to date when it shows again.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use super::{AppState, Effect};
use crate::dialogs::{Counting, Modal, PatternChoice, PatternDialog};
use crate::keys::Command;
use crate::pane::next_generation;
use crate::pattern::{NamePattern, PatternSyntax};
use crate::words;

/// The names of a pane's rows for a worker to match against a pattern
/// ([`MatchRequest::run`]), with the generation of the request.
#[derive(Clone, PartialEq, Eq)]
pub struct MatchRequest {
    pub generation: u64,
    pub rows: Arc<[OsString]>,
    pub pattern: NamePattern,
}

impl std::fmt::Debug for MatchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchRequest")
            .field("generation", &self.generation)
            .field("rows", &self.rows.len())
            .field("pattern", &self.pattern)
            .finish()
    }
}

/// How many names the worker matches between two looks at whether a newer request has come.
pub const MATCH_STRETCH: usize = 1_024;

impl MatchRequest {
    /// Matches the names. It takes long for many names: call it on a worker thread. `None`
    /// when `keep_going` says to stop (a newer request has come), which it is asked every
    /// [`MATCH_STRETCH`] names.
    pub fn run(self, keep_going: &dyn Fn() -> bool) -> Option<Matched> {
        let mut hits = Vec::new();
        for (i, name) in self.rows.iter().enumerate() {
            if i % MATCH_STRETCH == 0 && i > 0 && !keep_going() {
                return None;
            }
            if self.pattern.matches(name) {
                hits.push(i);
            }
        }
        Some(Matched {
            generation: self.generation,
            rows: self.rows,
            hits,
        })
    }
}

/// What a worker found for a [`MatchRequest`]: the positions of the names that match, among
/// its rows.
#[derive(Clone, PartialEq, Eq)]
pub struct Matched {
    pub generation: u64,
    pub rows: Arc<[OsString]>,
    pub hits: Vec<usize>,
}

impl std::fmt::Debug for Matched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Matched")
            .field("generation", &self.generation)
            .field("rows", &self.rows.len())
            .field("hits", &self.hits.len())
            .finish()
    }
}

/// What the pattern window works with besides itself.
#[derive(Debug, Default)]
pub(super) struct PatternWork {
    /// The names of the rows it selects among, as the worker was last given them; `None` until
    /// they are taken, and again after they changed.
    rows: Option<Arc<[OsString]>>,
    /// What the worker found for the window's pattern as it is.
    found: Option<Matched>,
    /// The choices of the last window confirmed, which the next one starts with.
    last: PatternChoice,
}

impl AppState {
    /// Whether the names of the files show their extensions (NAV-11).
    pub fn show_extensions(&self) -> bool {
        self.show_extensions
    }

    /// Shows or hides the extensions of the files' names in the panes, in every view. The
    /// names are as they are: a rename still edits the whole name.
    pub(super) fn toggle_extensions(&mut self) -> Vec<Effect> {
        self.show_extensions = !self.show_extensions;
        Vec::new()
    }

    /// F5 (macOS ⌘R), and 다시 시도 after a listing failed: lists the folder of pane `side`
    /// again under a new generation (NAV-13). The rows stay until the new listing replaces
    /// them, which keeps the selection of the names still there, the cursor on its name and
    /// the place the list is scrolled to. A sort a worker does for the old rows is dropped (the
    /// listing comes in the view's order), and navigation sync, if it waits for this listing,
    /// waits for the new one.
    pub(super) fn refresh(&mut self, side: usize) -> Vec<Effect> {
        let tab = self.tab_mut(side);
        let old = tab.generation();
        let request = tab.reload();
        self.sync_relisted(old, request.generation);
        vec![Effect::Load(request)]
    }

    /// The selection tool `command` on the rows of pane `side`: Ctrl+Space flips the cursor's
    /// row; 모두 선택, 선택 해제 (Esc) and 선택 반전 select all, none or the others; 패턴으로
    /// 선택… opens its window; 같은 확장자 선택 selects the rows like the cursor's.
    pub(super) fn select(&mut self, side: usize, command: &Command) -> Vec<Effect> {
        let (selection, rows) = self.tab_mut(side).selection_mut();
        match command {
            Command::ToggleSelect => {
                if let Some(cursor) = selection.cursor() {
                    selection.toggle(cursor, rows);
                }
            }
            Command::SelectAll => selection.select_all(rows),
            Command::ClearSelection => selection.clear(),
            Command::InvertSelection => selection.invert(rows),
            Command::SelectPattern => return self.open_pattern(side),
            Command::SelectSameExtension => return self.select_same_extension(side),
            _ => {}
        }
        Vec::new()
    }

    /// 같은 확장자 선택: the rows on screen with the extension of the cursor's row, ignoring
    /// case, are selected in place of the selection. A cursor on a row without one (a folder, a
    /// file such as `README`) selects nothing, and a notice says why.
    pub(super) fn select_same_extension(&mut self, side: usize) -> Vec<Effect> {
        let (selection, rows) = self.tab_mut(side).selection_mut();
        let Some(entry) = selection.cursor().and_then(|cursor| rows.get(cursor)) else {
            return Vec::new();
        };
        let Some(wanted) = entry.extension().map(folded) else {
            let name = entry.name.to_string_lossy();
            return vec![Effect::Notice(words::no_extension(&name))];
        };
        selection.select_where(rows, false, |_, row| {
            row.extension()
                .is_some_and(|extension| folded(extension) == wanted)
        });
        Vec::new()
    }

    /// 패턴으로 선택…: the pattern window for the rows of pane `side`, starting with the
    /// choices of the last one confirmed.
    pub(super) fn open_pattern(&mut self, side: usize) -> Vec<Effect> {
        let tab = self.tab(side).id();
        let dialog = PatternDialog::new(side, tab, self.pattern.last.clone());
        self.open(Modal::Pattern(dialog));
        self.pattern.rows = None;
        self.match_pattern(false)
    }

    /// The pattern window's field changed to `text`.
    pub fn pattern_edited(&mut self, text: &str) -> Vec<Effect> {
        self.change_pattern(|choice| text.clone_into(&mut choice.text))
    }

    /// The pattern window's 와일드카드 (`regex` false) or 정규식 (`regex`) was chosen.
    pub fn pattern_syntax(&mut self, regex: bool) -> Vec<Effect> {
        self.change_pattern(|choice| {
            choice.syntax = if regex {
                PatternSyntax::Regex
            } else {
                PatternSyntax::Wildcard
            };
        })
    }

    /// The pattern window's 대소문자 무시 was turned on (`ignore`) or off.
    pub fn pattern_case(&mut self, ignore: bool) -> Vec<Effect> {
        self.change_pattern(|choice| choice.match_case = !ignore)
    }

    /// The pattern window's 선택에 더하기 (`add`) or 선택 바꾸기 was chosen.
    pub fn pattern_mode(&mut self, add: bool) {
        if let Some(Modal::Pattern(dialog)) = &mut self.modal {
            dialog.choice.add = add;
        }
    }

    /// 선택 (or Enter) in the pattern window, whose field holds `text`: the rows the pattern
    /// matches are selected and the window closes, once the worker has matched them; a text
    /// that is no pattern keeps it open.
    pub fn pattern_confirmed(&mut self, text: &str) -> Vec<Effect> {
        let mut effects = self.pattern_edited(text);
        let Some(Modal::Pattern(dialog)) = &mut self.modal else {
            return effects;
        };
        if !dialog.can_confirm() {
            return effects;
        }
        let ready = self
            .pattern
            .found
            .as_ref()
            .is_some_and(|found| found.generation == dialog.generation);
        if ready {
            effects.extend(self.select_matched());
        } else if dialog.counting == Counting::Failed {
            effects.extend(self.match_here());
        } else {
            dialog.confirming = true;
        }
        effects
    }

    /// The worker could not match the names for request `generation` (it panicked, or no thread
    /// could be started for it): the window says it could not count them, and 선택 matches them
    /// on this thread when it is pressed (at once, when it was pressed already). An answer for a
    /// pattern typed over is dropped.
    pub fn pattern_failed(&mut self, generation: u64) -> Vec<Effect> {
        let Some(dialog) = self.pattern_window_mut() else {
            return Vec::new();
        };
        if generation != dialog.generation || dialog.counting != Counting::Running {
            return Vec::new();
        }
        dialog.counting = Counting::Failed;
        if dialog.confirming {
            self.match_here()
        } else {
            Vec::new()
        }
    }

    /// What the worker found for a [`MatchRequest`]: the window shows how many rows match, when
    /// it is still for the pattern as it is (an answer for an older one is dropped), and
    /// selects them when 선택 waits for it. A window waiting behind another one (the question
    /// whether to quit) takes it too.
    pub fn pattern_matched(&mut self, matched: Matched) -> Vec<Effect> {
        let Some(dialog) = self.pattern_window_mut() else {
            return Vec::new();
        };
        if matched.generation != dialog.generation || dialog.counting != Counting::Running {
            return Vec::new();
        }
        dialog.count = Some(matched.hits.len());
        dialog.counting = Counting::Idle;
        let confirming = dialog.confirming;
        self.pattern.found = Some(matched);
        if confirming {
            self.select_matched()
        } else {
            Vec::new()
        }
    }

    /// The rows of tab `index` of pane `side` changed (its listing came, its watch told of a
    /// change): when the pattern window selects among them, the worker matches them again. A
    /// 선택 that waited for the count waits for the new one.
    pub(super) fn pattern_rows_changed(&mut self, side: usize, index: usize) -> Vec<Effect> {
        let Some(dialog) = self.pattern_window() else {
            return Vec::new();
        };
        let id = self.panes[side].tabs().get(index).map(super::Tab::id);
        if dialog.side != side || id != Some(dialog.tab) {
            return Vec::new();
        }
        self.pattern.rows = None;
        self.match_pattern(true)
    }

    /// Drops what the pattern window worked with, once it is gone.
    pub(super) fn drop_closed_pattern(&mut self) {
        if self.pattern_window().is_none() {
            self.pattern.rows = None;
            self.pattern.found = None;
        }
    }

    /// The pattern window: the one that shows, or one that waits behind another window (the
    /// question whether to quit shows over it); there is one at most.
    fn pattern_window(&self) -> Option<&PatternDialog> {
        self.modal
            .iter()
            .chain(&self.later)
            .find_map(|modal| match modal {
                Modal::Pattern(dialog) => Some(dialog),
                _ => None,
            })
    }

    fn pattern_window_mut(&mut self) -> Option<&mut PatternDialog> {
        self.modal
            .iter_mut()
            .chain(&mut self.later)
            .find_map(|modal| match modal {
                Modal::Pattern(dialog) => Some(dialog),
                _ => None,
            })
    }

    /// Changes the pattern window's choices with `change`; the worker matches the names
    /// again when the pattern changed.
    fn change_pattern(&mut self, change: impl FnOnce(&mut PatternChoice)) -> Vec<Effect> {
        let Some(Modal::Pattern(dialog)) = &mut self.modal else {
            return Vec::new();
        };
        let before = dialog.choice.clone();
        change(&mut dialog.choice);
        if dialog.choice == before {
            return Vec::new();
        }
        self.match_pattern(false)
    }

    /// Asks a worker to match the names of the pattern window's rows against its pattern as it
    /// is, under a new generation; or, for a text that is no pattern or has nothing to match,
    /// says so in the window and asks nothing. A 선택 that waited for the count waits for the
    /// new one when `keep_choice` (the rows changed), and is dropped otherwise (the pattern
    /// changed after it).
    fn match_pattern(&mut self, keep_choice: bool) -> Vec<Effect> {
        let Some(dialog) = self.pattern_window() else {
            return Vec::new();
        };
        let (side, tab) = (dialog.side, dialog.tab);
        let rows = if let Some(rows) = &self.pattern.rows {
            Arc::clone(rows)
        } else {
            let names: Arc<[OsString]> = self.panes[side]
                .position(tab)
                .map(|index| &self.panes[side].tabs()[index])
                .map(|tab| {
                    tab.visible()
                        .iter()
                        .map(|entry| entry.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            self.pattern.rows = Some(Arc::clone(&names));
            names
        };
        self.pattern.found = None;
        let Some(dialog) = self.pattern_window_mut() else {
            return Vec::new();
        };
        dialog.generation = next_generation();
        dialog.confirming &= keep_choice;
        let (pattern, problem) = match dialog.choice.pattern() {
            Ok(pattern) => (pattern, None),
            Err(problem) => (None, Some(problem)),
        };
        dialog.problem = problem;
        dialog.empty = problem.is_none() && pattern.is_none();
        dialog.counting = if pattern.is_some() {
            Counting::Running
        } else {
            Counting::Idle
        };
        if pattern.is_none() {
            dialog.count = None;
        }
        pattern
            .map(|pattern| {
                Effect::MatchNames(MatchRequest {
                    generation: dialog.generation,
                    rows,
                    pattern,
                })
            })
            .into_iter()
            .collect()
    }

    /// Matches the names of the pattern window's rows on this thread, which the worker could not
    /// do, and selects the rows that match ([`AppState::select_matched`]).
    fn match_here(&mut self) -> Vec<Effect> {
        let Some(dialog) = self.pattern_window() else {
            return Vec::new();
        };
        let Ok(Some(pattern)) = dialog.choice.pattern() else {
            return Vec::new();
        };
        let request = MatchRequest {
            generation: dialog.generation,
            rows: self.pattern.rows.clone().unwrap_or_default(),
            pattern,
        };
        self.pattern.found = request.run(&|| true);
        self.select_matched()
    }

    /// Selects the rows the worker matched, in the tab the pattern window was opened for (if it
    /// is still in its pane), by their names: in place of the selection or with it. The window
    /// closes (or, waiting behind another one, goes), and the next one starts with its choices.
    fn select_matched(&mut self) -> Vec<Effect> {
        let Some(dialog) = self.pattern_window() else {
            return Vec::new();
        };
        let (side, tab, choice) = (dialog.side, dialog.tab, dialog.choice.clone());
        if let Some(found) = self.pattern.found.take()
            && let Some(index) = self.panes[side].position(tab)
        {
            let names: HashSet<&OsStr> = found
                .hits
                .iter()
                .filter_map(|&hit| found.rows.get(hit))
                .map(OsString::as_os_str)
                .collect();
            let (selection, rows) = self.panes[side].tabs_mut()[index].selection_mut();
            selection.select_where(rows, choice.add, |_, row| {
                names.contains(row.name.as_os_str())
            });
        }
        self.pattern.last = choice;
        if matches!(self.modal, Some(Modal::Pattern(_))) {
            self.close();
        } else {
            self.later
                .retain(|modal| !matches!(modal, Modal::Pattern(_)));
            self.drop_closed_pattern();
        }
        Vec::new()
    }
}

/// An extension as it is compared: in lowercase.
fn folded(extension: &OsStr) -> String {
    extension.to_string_lossy().to_lowercase()
}
