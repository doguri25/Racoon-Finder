//! The preview panel (VIEW-02) and quick look (VIEW-01). A child of [`state`](super), so these
//! methods see the fields of [`AppState`].
//!
//! Both show one item: the row under the cursor of the active pane's tab, in any view mode and
//! with one pane or two. That is the cursor the list placed too: a preview only reads, so the
//! rule that keeps the commands that change files off a row the user has not chosen
//! (`Selection::user_cursor`) does not apply, and the panel shows something as soon as a folder
//! is listed. Nothing is asked while neither shows.
//!
//! [`AppState::follow_preview`] (the controller calls it after every change, as it does
//! [`AppState::follow_active`]) compares that item with the one shown: its path, kind, size and
//! time, and which items a folder counts. When it is another one, it gets a new generation and
//! a worker is asked for its content ([`Effect::Preview`]), which replaces and stops the one
//! asked for before; [`AppState::previewed`] takes only the answer with the generation shown. A
//! link or a special file shows its facts alone, without a worker.
//!
//! Quick look opens on Space (and ⌘Y on macOS; while a name is being typed Space types, see
//! `keys::pane_command`) over the item, and takes every key while it shows
//! ([`AppState::quick_look_key`]): ← and → put the cursor on the file before or after, in the
//! order of the rows, past the folders (a folder it opened on shows its count); Space, Esc and
//! ⌘Y close it. It closes too when there is nothing left to show, when a window opens over the
//! main window, and when the active pane, its tab or its folder changes.

use std::time::SystemTime;

use rf_core::{Entry, EntryKind, VPath};
use rf_vfs::VisibilityFilter;

use super::{AppState, Effect};
use crate::keys::QuickLookKey;
use crate::pane::next_generation;
use crate::preview::{PreviewContent, PreviewRequest, Previewed};
use crate::selection::Pick;

/// What the preview shows, and where quick look opened.
#[derive(Debug, Default)]
pub(super) struct PreviewWork {
    shown: Option<Shown>,
    quick_look: Option<QuickLookAt>,
}

/// The item the preview shows.
#[derive(Debug)]
struct Shown {
    target: Target,
    entry: Entry,
    generation: u64,
    /// `None` while a worker makes it.
    content: Option<PreviewContent>,
}

/// What makes an item another one for the preview: a change of any of these is previewed anew.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    path: VPath,
    kind: EntryKind,
    size: u64,
    modified: Option<SystemTime>,
    filter: VisibilityFilter,
}

/// The tab quick look opened in: pane `side`'s tab `tab` at `folder`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct QuickLookAt {
    side: usize,
    tab: u64,
    folder: VPath,
}

/// What the preview panel and quick look show ([`AppState::preview`]).
#[derive(Clone, Copy, Debug)]
pub struct PreviewShown<'a> {
    pub entry: &'a Entry,
    /// `None` while a worker makes it.
    pub content: Option<&'a PreviewContent>,
    /// The generation of the item's request: a new one for every item shown.
    pub generation: u64,
}

impl AppState {
    /// What the preview panel and quick look show, while one of them does and the active pane
    /// has a cursor.
    pub fn preview(&self) -> Option<PreviewShown<'_>> {
        self.preview.shown.as_ref().map(|shown| PreviewShown {
            entry: &shown.entry,
            content: shown.content.as_ref(),
            generation: shown.generation,
        })
    }

    /// Whether quick look shows.
    pub fn quick_look_open(&self) -> bool {
        self.preview.quick_look.is_some()
    }

    /// Whether a name is being typed to select in the active pane at `now`
    /// ([`Tab::typing`](crate::pane::Tab::typing)): a Space then types.
    pub fn typing(&self, now: std::time::Instant) -> bool {
        self.tab(self.active).typing(now)
    }

    /// Makes the preview follow the active pane's cursor (module docs): the request for the
    /// item it is on now, when that is another one and the panel or quick look shows, or the
    /// end of the request under way when there is nothing to show any more.
    pub fn follow_preview(&mut self) -> Vec<Effect> {
        if self.quick_look_strays() {
            self.preview.quick_look = None;
        }
        let wanted = self.layout.preview || self.preview.quick_look.is_some();
        let Some((target, entry)) = self.preview_target().filter(|_| wanted) else {
            // Quick look has nothing to show.
            self.preview.quick_look = None;
            return match self.preview.shown.take() {
                Some(shown) if shown.content.is_none() => vec![Effect::StopPreview],
                _ => Vec::new(),
            };
        };
        if let Some(shown) = &mut self.preview.shown
            && shown.target == target
        {
            // The same item; its other facts (a new attribute) show as they are now.
            shown.entry = entry;
            return Vec::new();
        }
        let generation = next_generation();
        let mut effects = Vec::new();
        let content = if matches!(target.kind, EntryKind::File | EntryKind::Dir) {
            effects.push(Effect::Preview(PreviewRequest {
                generation,
                path: target.path.clone(),
                kind: target.kind,
                filter: target.filter,
            }));
            None
        } else {
            // The request for the item before is of no use now.
            if self
                .preview
                .shown
                .as_ref()
                .is_some_and(|shown| shown.content.is_none())
            {
                effects.push(Effect::StopPreview);
            }
            Some(PreviewContent::Plain)
        };
        self.preview.shown = Some(Shown {
            target,
            entry,
            generation,
            content,
        });
        effects
    }

    /// Takes what a worker made of a request ([`Effect::Preview`]): only the one for the item
    /// shown, by its generation.
    pub fn previewed(&mut self, previewed: Previewed) -> Vec<Effect> {
        if let Some(shown) = &mut self.preview.shown
            && shown.generation == previewed.generation
            && shown.content.is_none()
        {
            shown.content = Some(previewed.content);
        }
        Vec::new()
    }

    /// A key while quick look shows ([`keys::quick_look_key`](crate::keys::quick_look_key)).
    pub fn quick_look_key(&mut self, key: QuickLookKey) -> Vec<Effect> {
        match key {
            QuickLookKey::Close => {
                self.close_quick_look();
                Vec::new()
            }
            QuickLookKey::Previous => self.quick_look_step(false),
            QuickLookKey::Next => self.quick_look_step(true),
        }
    }

    /// Closes quick look (its close button, a click beside it, its keys).
    pub fn close_quick_look(&mut self) {
        self.preview.quick_look = None;
    }

    /// Space (⌘Y): opens quick look over the cursor's row of pane `side`, or closes it. Nothing
    /// to show (no row), nothing opens.
    pub(super) fn toggle_quick_look(&mut self, side: usize) -> Vec<Effect> {
        if self.preview.quick_look.take().is_some() {
            return Vec::new();
        }
        let tab = self.tab(side);
        if tab.selection().cursor().is_some() {
            self.preview.quick_look = Some(QuickLookAt {
                side,
                tab: tab.id(),
                folder: tab.location().clone(),
            });
        }
        Vec::new()
    }

    /// ← or → in quick look: the cursor goes to the file before or after its row, past the
    /// folders, and that file alone is selected (as the arrows of the list would select it); at
    /// the first or last file it stays.
    fn quick_look_step(&mut self, forward: bool) -> Vec<Effect> {
        let Some(side) = self.preview.quick_look.as_ref().map(|at| at.side) else {
            return Vec::new();
        };
        let (selection, rows) = self.tab_mut(side).selection_mut();
        let Some(from) = selection.cursor() else {
            return Vec::new();
        };
        let file = |i: &usize| rows[*i].kind != EntryKind::Dir;
        let found = if forward {
            (from + 1..rows.len()).find(file)
        } else {
            (0..from).rev().find(file)
        };
        let Some(row) = found else {
            return Vec::new();
        };
        selection.go_to(row, Pick::Select, rows);
        vec![Effect::ScrollTo { side, row }]
    }

    /// Whether quick look is to close: a window shows over the main window, the app ends, or
    /// the active pane, its tab or its folder is not the one it opened in.
    fn quick_look_strays(&self) -> bool {
        let Some(at) = &self.preview.quick_look else {
            return false;
        };
        let tab = self.tab(self.active);
        self.modal.is_some()
            || self.quitting
            || at.side != self.active
            || at.tab != tab.id()
            || at.folder != *tab.location()
    }

    /// The item under the active pane's cursor, and what makes it the one it is.
    fn preview_target(&self) -> Option<(Target, Entry)> {
        let tab = self.tab(self.active);
        let entry = tab.visible().get(tab.selection().cursor()?)?;
        let target = Target {
            path: tab.location().join(&entry.name).ok()?,
            kind: entry.kind,
            size: entry.size,
            modified: entry.modified,
            filter: tab.filter(),
        };
        Some((target, entry.clone()))
    }
}
