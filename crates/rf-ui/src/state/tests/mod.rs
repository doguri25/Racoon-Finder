//! Tests of [`AppState`](super::AppState): the panes, the keys and the tree ([`nav`]), the
//! file commands, the jobs and the dialogs ([`jobs`]), dragging rows ([`drag`]), and the layout
//! and the commands between the panes ([`layout`]), the tree from the keyboard ([`sidebar`]),
//! the tabs ([`tabs`]), with the helpers they share.

use super::*;
use crate::keys;
use crate::places::PlaceKind;
use rf_core::{Attributes, Entry, EntryKind, Os};
use rf_jobs::{JobId, JobReport};
use rf_vfs::SortOrder;
use slint::platform::Key;
use std::sync::Arc;

mod copies;
mod drag;
mod jobs;
mod layout;
mod nav;
mod pick;
mod session;
mod sidebar;
mod tabs;

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

/// `effects` without the read of the free space that the end of every job asks for (a test
/// of its own looks at that).
fn without_space_reads(effects: Vec<Effect>) -> Vec<Effect> {
    effects
        .into_iter()
        .filter(|effect| !matches!(effect, Effect::ReadSpace { .. }))
        .collect()
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
    let command = keys::command_for(text, mods, Os::Windows).expect("a shortcut");
    state.key(command, mods, 10, Instant::now())
}

fn special(key: Key) -> String {
    char::from(key).to_string()
}

const NONE: Mods = Mods::NONE;

const SHIFT: Mods = Mods {
    shift: true,
    ..NONE
};

const CTRL: Mods = Mods { ctrl: true, ..NONE };

const ALT: Mods = Mods { alt: true, ..NONE };

const CTRL_SHIFT: Mods = Mods {
    ctrl: true,
    shift: true,
    ..NONE
};
