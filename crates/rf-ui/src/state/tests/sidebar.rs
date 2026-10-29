//! The sidebar's folder tree from the keyboard: F6 gives it the keys, its cursor moves, opens
//! and closes rows, Enter shows a folder in the active pane, and Esc gives the keys back.

use super::nav::{drive, temp, the_tree_load};
use super::*;
use crate::keys::TreeKey;

/// A window whose tree shows the place 임시 (the temporary folder, which holds both panes'
/// folders) open on its folders a, b and c; the left pane's a is selected.
fn with_tree() -> AppState {
    let mut state = ready(vec![], vec![]);
    let mut effects = state.set_places(&[drive("임시", &temp())]);
    effects.extend(state.follow_active());
    let request = the_tree_load(&effects);
    let _ = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("a"), dir("b"), dir("c")],
    });
    assert_eq!(state.tree().selected_row(), Some(1));
    state
}

fn labels(state: &AppState) -> Vec<String> {
    state
        .tree()
        .rows()
        .into_iter()
        .map(|row| row.label)
        .collect()
}

#[test]
fn f6_gives_the_tree_the_keys_on_the_active_pane_s_folder() {
    let mut state = with_tree();
    assert_eq!(state.tree_cursor_row(), None);

    let effects = press(&mut state, &special(Key::F6), NONE).expect("a shortcut");

    assert_eq!(effects, [Effect::FocusTree, Effect::ScrollTree(1)]);
    assert_eq!(state.tree_cursor_row(), Some(1));
    // From the right pane, on its folder.
    assert_eq!(state.tree_key(TreeKey::Leave), [Effect::FocusPanes]);
    press(&mut state, &special(Key::Tab), NONE);
    let _ = state.follow_active();
    press(&mut state, &special(Key::F6), NONE);
    assert_eq!(state.tree_cursor_row(), Some(2));
}

#[test]
fn the_arrows_move_open_and_close_in_the_tree() {
    let mut state = with_tree();
    press(&mut state, &special(Key::F6), NONE);

    // ↓ goes down to c, and no further.
    assert_eq!(state.tree_key(TreeKey::Down), [Effect::ScrollTree(2)]);
    state.tree_key(TreeKey::Down);
    state.tree_key(TreeKey::Down);
    assert_eq!(state.tree_cursor_row(), Some(3));
    // ↑ goes up to the place, and no further.
    for _ in 0..5 {
        state.tree_key(TreeKey::Up);
    }
    assert_eq!(state.tree_cursor_row(), Some(0));

    // ← on the open place closes it; → opens it again, listed already.
    state.tree_key(TreeKey::Collapse);
    assert_eq!(labels(&state), ["임시"]);
    assert_eq!(state.tree_cursor_row(), Some(0));
    assert_eq!(state.tree_key(TreeKey::Expand), [Effect::ScrollTree(0)]);
    assert_eq!(labels(&state), ["임시", "a", "b", "c"]);
    // → on an open row goes to its first folder.
    state.tree_key(TreeKey::Expand);
    assert_eq!(state.tree_cursor_row(), Some(1));
    // → on a folder never listed opens and lists it; then goes into it.
    let request = the_tree_load(&state.tree_key(TreeKey::Expand));
    assert_eq!(request.path, folder("a"));
    let _ = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("inner")],
    });
    assert_eq!(labels(&state), ["임시", "a", "inner", "b", "c"]);
    state.tree_key(TreeKey::Expand);
    assert_eq!(state.tree_cursor_row(), Some(2));
    // ← on a closed folder goes to the folder above it; ← again closes that one.
    state.tree_key(TreeKey::Collapse);
    assert_eq!(state.tree_cursor_row(), Some(1));
    state.tree_key(TreeKey::Collapse);
    assert_eq!(labels(&state), ["임시", "a", "b", "c"]);
    assert_eq!(state.tree_cursor_row(), Some(1));
    // Moves alone show no folder in a pane.
    assert_eq!(state.panes()[LEFT].active_tab().location(), &folder("a"));
}

#[test]
fn enter_shows_the_folder_in_the_active_pane_and_the_tree_keeps_the_keys() {
    let mut state = with_tree();
    press(&mut state, &special(Key::Tab), NONE);
    let _ = state.follow_active();
    press(&mut state, &special(Key::F6), NONE);
    state.tree_key(TreeKey::Down);

    let effects = state.tree_key(TreeKey::Open);

    assert_eq!(the_load(&effects).path, folder("c"));
    assert!(!effects.contains(&Effect::FocusPanes));
    assert_eq!(state.active(), RIGHT);
    assert_eq!(state.panes()[RIGHT].active_tab().location(), &folder("c"));
    assert_eq!(state.tree().selected_row(), Some(3));
    assert_eq!(state.tree_cursor_row(), Some(3));
    // Esc, Tab and F6 give the keys back to the pane.
    assert_eq!(state.tree_key(TreeKey::Leave), [Effect::FocusPanes]);
}

#[test]
fn a_hidden_tree_gives_the_keys_back_and_f6_shows_it() {
    let mut state = with_tree();
    press(&mut state, &special(Key::F6), NONE);

    assert_eq!(press(&mut state, "b", CTRL), Some(vec![Effect::FocusPanes]));
    assert!(!state.layout().tree);

    let effects = press(&mut state, &special(Key::F6), NONE).expect("a shortcut");
    assert!(state.layout().tree);
    assert!(effects.contains(&Effect::FocusTree));
    assert_eq!(state.tree_cursor_row(), Some(1));
}

#[test]
fn keys_in_an_empty_tree_do_nothing() {
    let mut state = ready(vec![], vec![]);
    assert_eq!(
        press(&mut state, &special(Key::F6), NONE),
        Some(vec![Effect::FocusTree])
    );
    for key in [
        TreeKey::Up,
        TreeKey::Down,
        TreeKey::Expand,
        TreeKey::Collapse,
        TreeKey::Open,
    ] {
        assert_eq!(state.tree_key(key), [], "{key:?}");
    }
    assert_eq!(state.tree_key(TreeKey::Leave), [Effect::FocusPanes]);
}
