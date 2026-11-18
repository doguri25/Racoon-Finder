//! The folder tree's three modes (LAY-03): the shared tree that follows the active pane, a tree
//! for each pane that follows that pane alone and opens and closes on its own, and none; F6
//! through both trees; a click, a key and a drop in the second tree; and the toolbar that
//! comes and goes (LAY-10).

use super::nav::{drive, temp, the_tree_load};
use super::panel::running;
use super::*;
use crate::keys::TreeKey;

/// The one listing of folder tree `tree` among `effects`, listed with `entries`.
fn list_tree(state: &mut AppState, effects: &[Effect], entries: Vec<Entry>) {
    let request = the_tree_load(effects);
    let _ = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries,
    });
}

/// The labels of the rows of tree `tree`.
fn labels(state: &AppState, tree: usize) -> Vec<String> {
    state.trees()[tree]
        .rows()
        .into_iter()
        .map(|row| row.label)
        .collect()
}

/// A window whose panes show a and b of the place 임시 (the temporary folder, which holds a, b
/// and c), with a tree for each pane, each listed and on its pane's folder.
fn per_pane() -> AppState {
    let mut state = ready(vec![], vec![]);
    let mut effects = state.set_places(&[drive("임시", &temp())]);
    effects.extend(state.set_tree_mode(TreeMode::PerPane));
    effects.extend(state.follow_active());
    // Each tree lists the place on its own: two listings, one for each.
    let requests: Vec<LoadRequest> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LoadChildren(request) => Some(request.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 2, "{effects:?}");
    for request in requests {
        assert_eq!(request.path, temp());
        let _ = state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![dir("a"), dir("b"), dir("c")],
        });
    }
    assert_eq!(state.trees()[LEFT].selected_row(), Some(1));
    assert_eq!(state.trees()[RIGHT].selected_row(), Some(2));
    state
}

#[test]
fn the_shared_tree_follows_the_active_pane_and_a_pane_s_tree_its_own_pane() {
    // Shared: one tree, on the active pane's folder; the second one does not follow.
    let mut state = ready(vec![], vec![]);
    let mut effects = state.set_places(&[drive("임시", &temp())]);
    effects.extend(state.follow_active());
    list_tree(&mut state, &effects, vec![dir("a"), dir("b"), dir("c")]);
    assert_eq!(state.layout().tree, TreeMode::Shared);
    assert_eq!(state.layout().shown_trees(LEFT), [LEFT]);
    assert_eq!(state.tree().selected_row(), Some(1));
    press(&mut state, &special(Key::Tab), NONE);
    assert_eq!(
        state.follow_active(),
        [Effect::ScrollTree { tree: 0, row: 2 }]
    );
    assert_eq!(
        state.trees()[RIGHT].target(),
        None,
        "the second tree does not show"
    );

    // Each pane's own: each tree on its pane's folder, whichever pane is active.
    let mut state = per_pane();
    assert_eq!(state.layout().shown_trees(LEFT), [LEFT, RIGHT]);
    assert_eq!(state.tree_pane(LEFT), LEFT);
    assert_eq!(state.tree_pane(RIGHT), RIGHT);
    press(&mut state, &special(Key::Tab), NONE);
    assert_eq!(state.active(), RIGHT);
    assert_eq!(state.follow_active(), []);
    // The right pane goes to c: its tree follows, the left one stays on a.
    let _ = state.go_to(folder("c"));
    assert_eq!(
        state.follow_active(),
        [Effect::ScrollTree { tree: 1, row: 3 }]
    );
    assert_eq!(state.trees()[LEFT].selected_row(), Some(1));
    assert_eq!(state.trees()[RIGHT].selected_row(), Some(3));
    // The left pane goes to b: its tree alone follows.
    state.activate(LEFT);
    let _ = state.go_to(folder("b"));
    assert_eq!(
        state.follow_active(),
        [Effect::ScrollTree { tree: 0, row: 2 }]
    );
    assert_eq!(state.trees()[RIGHT].selected_row(), Some(3));

    // Back to the shared tree: tree 0, which keeps what it had open, follows the active pane.
    state.activate(RIGHT);
    assert_eq!(state.set_tree_mode(TreeMode::Shared), []);
    assert_eq!(
        state.follow_active(),
        [Effect::ScrollTree { tree: 0, row: 3 }]
    );
}

#[test]
fn with_one_pane_the_tree_that_shows_is_that_pane_s_own() {
    let mut state = per_pane();
    press(&mut state, &special(Key::F12), NONE);
    assert!(!state.layout().dual);
    assert_eq!(state.layout().shown_trees(LEFT), [LEFT]);
    // The right pane shows alone: its own tree with it, on its own folder.
    press(&mut state, &special(Key::Tab), NONE);
    assert_eq!(state.active(), LEFT, "Tab stays on the one pane");
    state.activate(RIGHT);
    assert_eq!(state.layout().shown_trees(RIGHT), [RIGHT]);
    let _ = state.go_to(folder("c"));
    assert_eq!(
        state.follow_active(),
        [Effect::ScrollTree { tree: 1, row: 3 }]
    );
    // The tree of the pane that is put away does not follow it.
    assert_eq!(state.trees()[LEFT].selected_row(), Some(1));
}

#[test]
fn opening_and_closing_in_one_tree_leaves_the_other_as_it_is() {
    let mut state = per_pane();
    let both = vec!["임시", "a", "b", "c"];
    assert_eq!(labels(&state, LEFT), both);
    assert_eq!(labels(&state, RIGHT), both);

    // The place closes in the left tree only.
    assert_eq!(state.toggle_tree_row(LEFT, 0), []);
    assert_eq!(labels(&state, LEFT), ["임시"]);
    assert_eq!(labels(&state, RIGHT), both);

    // c opens in the right tree only: its listing goes to that tree, on the loader's workers.
    let effects = state.toggle_tree_row(RIGHT, 3);
    let request = the_tree_load(&effects);
    assert_eq!(request.path, folder("c"));
    let _ = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("d")],
    });
    assert_eq!(labels(&state, RIGHT), ["임시", "a", "b", "c", "d"]);
    assert_eq!(labels(&state, LEFT), ["임시"]);
    // Opened again in the left tree, the place shows its folders as they were, c closed.
    assert_eq!(state.toggle_tree_row(LEFT, 0), []);
    assert_eq!(labels(&state, LEFT), both);
    // A row that is not there does nothing, nor does a third tree.
    assert_eq!(state.toggle_tree_row(RIGHT, 99), []);
    assert_eq!(state.toggle_tree_row(2, 0), []);
    assert_eq!(state.open_tree_row(2, 0), []);
}

#[test]
fn a_click_in_a_pane_s_tree_moves_that_pane_and_gives_it_the_keyboard() {
    let mut state = per_pane();
    assert_eq!(state.active(), LEFT);

    let effects = state.open_tree_row(RIGHT, 3);

    assert_eq!(the_load(&effects).path, folder("c"));
    assert_eq!(state.active(), RIGHT);
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), folder("c"));
    assert_eq!(*state.panes()[LEFT].active_tab().location(), folder("a"));
    assert_eq!(state.follow_active(), []);
    assert_eq!(state.trees()[LEFT].selected_row(), Some(1));

    // The shared tree moves the active pane.
    let _ = state.set_tree_mode(TreeMode::Shared);
    state.activate(LEFT);
    let effects = state.open_tree_row(LEFT, 2);
    assert_eq!(the_load(&effects).path, folder("b"));
    assert_eq!(state.active(), LEFT);
}

#[test]
fn f6_goes_through_both_trees_then_on_to_the_panel_or_the_panes() {
    let mut state = per_pane();

    let effects = press(&mut state, &special(Key::F6), NONE).expect("a shortcut");
    assert_eq!(
        effects,
        [Effect::FocusTree(0), Effect::ScrollTree { tree: 0, row: 1 }]
    );
    assert_eq!(state.tree_with_keys(), LEFT);
    // F6 in the left pane's tree: the right pane's, on its own pane's folder.
    assert_eq!(
        state.tree_key(TreeKey::Next),
        [Effect::FocusTree(1), Effect::ScrollTree { tree: 1, row: 2 }]
    );
    assert_eq!(state.tree_with_keys(), RIGHT);
    assert_eq!(state.tree_cursor_row(), Some(2));
    // Its keys move its own cursor, and Enter shows the folder in its pane.
    assert_eq!(
        state.tree_key(TreeKey::Down),
        [Effect::ScrollTree { tree: 1, row: 3 }]
    );
    let effects = state.tree_key(TreeKey::Open);
    assert_eq!(the_load(&effects).path, folder("c"));
    assert_eq!(state.active(), RIGHT);
    assert_eq!(state.trees()[LEFT].selected_row(), Some(1));
    // Then the panes, without a job panel.
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanes]);

    // With a job, the panel after the second tree.
    running(&mut state, 1, "x");
    press(&mut state, &special(Key::F6), NONE);
    state.tree_key(TreeKey::Next);
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanel]);

    // With the shared tree, F6 from it goes on at once.
    let mut state = per_pane();
    let _ = state.set_tree_mode(TreeMode::Shared);
    press(&mut state, &special(Key::F6), NONE);
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanes]);

    // With one pane, the right one: its own tree is the one F6 visits.
    let mut state = per_pane();
    press(&mut state, &special(Key::F12), NONE);
    state.activate(RIGHT);
    let effects = press(&mut state, &special(Key::F6), NONE).expect("a shortcut");
    assert_eq!(effects[0], Effect::FocusTree(1));
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanes]);
}

#[test]
fn the_tree_the_window_says_has_the_keys_takes_them_and_one_that_went_gives_them_back() {
    let mut state = per_pane();
    press(&mut state, &special(Key::F6), NONE);
    assert_eq!(state.tree_cursor_row(), Some(1));
    // The window's keys came from the right pane's tree: its cursor starts anew there.
    assert_eq!(state.tree_has_keys(RIGHT), []);
    assert_eq!(state.tree_with_keys(), RIGHT);
    assert_eq!(state.tree_cursor_row(), None);
    assert_eq!(
        state.tree_key(TreeKey::Down),
        [Effect::ScrollTree { tree: 1, row: 3 }]
    );
    // The same tree again changes nothing.
    assert_eq!(state.tree_has_keys(RIGHT), []);
    assert_eq!(state.tree_cursor_row(), Some(3));
    // A tree that does not show (the shared tree has no second one) gives the keys back.
    let _ = state.set_tree_mode(TreeMode::Shared);
    assert_eq!(state.tree_has_keys(RIGHT), [Effect::FocusPanes]);
    let _ = state.set_tree_mode(TreeMode::Hidden);
    assert_eq!(state.tree_has_keys(LEFT), [Effect::FocusPanes]);
}

#[test]
fn ctrl_b_hides_the_trees_and_brings_them_back_as_they_showed() {
    let mut state = per_pane();
    assert_eq!(press(&mut state, "b", CTRL), Some(vec![Effect::FocusPanes]));
    assert_eq!(state.layout().tree, TreeMode::Hidden);
    assert_eq!(state.layout().shown_trees(LEFT), Vec::<usize>::new());
    assert_eq!(state.follow_active(), []);
    // F6 passes them by.
    assert_eq!(
        press(&mut state, &special(Key::F6), NONE),
        Some(vec![Effect::FocusPanes])
    );
    assert_eq!(press(&mut state, "b", CTRL), Some(vec![]));
    assert_eq!(state.layout().tree, TreeMode::PerPane);
    // The 보기 menu's 트리 숨김, then Ctrl+B: the mode it showed in last.
    let _ = state.set_tree_mode(TreeMode::Shared);
    assert_eq!(state.set_tree_mode(TreeMode::Hidden), [Effect::FocusPanes]);
    press(&mut state, "b", CTRL);
    assert_eq!(state.layout().tree, TreeMode::Shared);

    // A start with the tree hidden brings back the shared one.
    let mut session = Session::default();
    session.layout.tree = TreeMode::Hidden;
    let (mut state, _) = AppState::restore(folder("a"), folder("b"), session, &[], None);
    press(&mut state, "b", CTRL);
    assert_eq!(state.layout().tree, TreeMode::Shared);
    // A start with a tree for each pane brings that back.
    let mut session = Session::default();
    session.layout.tree = TreeMode::PerPane;
    let (mut state, _) = AppState::restore(folder("a"), folder("b"), session, &[], None);
    press(&mut state, "b", CTRL);
    press(&mut state, "b", CTRL);
    assert_eq!(state.layout().tree, TreeMode::PerPane);
}

#[test]
fn a_drop_on_a_row_of_the_right_pane_s_tree_goes_into_that_row_s_folder() {
    let mut state = per_pane();
    let _ = state.toggle_tree_row(LEFT, 0);
    // Items of the left pane, readied for a drag. Its folder a shows a file.
    let request = the_load(&state.go_to(folder("a")));
    let _ = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![file("x.txt")],
    });
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    let offer = |state: &mut AppState, tree, row| {
        state
            .drop_offer(id, DropSpot::Tree { tree, row })
            .map(|offer| offer.folder)
    };

    // Row 3 is c in the right tree; the left one, closed, has no row 3.
    assert_eq!(offer(&mut state, RIGHT, 3), Some(folder("c")));
    assert_eq!(offer(&mut state, LEFT, 3), None);
    assert_eq!(offer(&mut state, LEFT, 0), Some(temp()));
    // A rest on a closed row opens it in that tree only.
    let _ = state.toggle_tree_row(RIGHT, 0);
    let spot = DropSpot::Tree { tree: LEFT, row: 0 };
    let _ = state.drop_hover(id, spot);
    assert_eq!(state.drag_rests(spot), []);
    assert_eq!(labels(&state, LEFT), ["임시", "a", "b", "c"]);
    assert_eq!(labels(&state, RIGHT), ["임시"]);
}

#[test]
fn the_toolbar_hides_and_the_filter_s_key_brings_it_back() {
    let mut state = ready(vec![file("x.txt")], vec![]);
    assert!(state.layout().toolbar);
    assert_eq!(state.toggle_toolbar(), []);
    assert!(!state.layout().toolbar);
    // The other keys leave it hidden.
    press(&mut state, &special(Key::F10), NONE);
    assert!(!state.layout().toolbar);
    // The filter box is in it: its key shows it again, with the keys in the box.
    assert_eq!(
        press(&mut state, "f", CTRL),
        Some(vec![Effect::FocusFilter])
    );
    assert!(state.layout().toolbar);
    assert_eq!(state.toggle_toolbar(), []);
    assert_eq!(state.toggle_toolbar(), []);
    assert!(state.layout().toolbar);
}

#[test]
fn a_favorite_s_menu_in_the_right_pane_s_tree_opens_in_that_pane() {
    let mut state = ready(vec![], vec![]);
    let _ = state.set_places(&[Place {
        label: "작업".to_owned(),
        path: folder("c"),
        kind: PlaceKind::Favorite(rf_platform::KnownFolder::Documents),
        space: None,
    }]);
    let _ = state.set_tree_mode(TreeMode::PerPane);
    // 열기 in the right pane's tree: the right pane shows the favorite, and has the keys.
    state.activate_tree_pane(RIGHT);
    let effects = state.favorite_command(0, "open");
    assert_eq!(the_load(&effects).path, folder("c"));
    assert_eq!(state.active(), RIGHT);
    assert_eq!(*state.panes()[LEFT].active_tab().location(), folder("a"));
    // 반대 패널에 열기 there: the left pane.
    let effects = state.favorite_command(0, "open-other");
    assert_eq!(the_load(&effects).path, folder("c"));
    assert_eq!(*state.panes()[LEFT].active_tab().location(), folder("c"));
    // The shared tree's menus act on the active pane, as before.
    let _ = state.set_tree_mode(TreeMode::Shared);
    state.activate(LEFT);
    state.activate_tree_pane(LEFT);
    assert_eq!(state.active(), LEFT);
    state.activate(RIGHT);
    state.activate_tree_pane(LEFT);
    assert_eq!(state.active(), RIGHT);
}
