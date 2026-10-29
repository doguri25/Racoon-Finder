//! The layout of the window and the commands between the two panes: one pane or two, the
//! split and its divider, the tree and the preview panel's place, swapping the panes, the same
//! folder in the other pane, and navigation sync.

use super::nav::{drive, temp, the_tree_load};
use super::*;
use crate::dialogs::Modal;
use crate::settings::Session;

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn in_b(name: &str) -> VPath {
    folder("b").join(name).expect("a plain name")
}

/// The folders of the tabs of pane `side`, in order.
fn locations(state: &AppState, side: usize) -> Vec<VPath> {
    state.panes()[side]
        .tabs()
        .iter()
        .map(|tab| tab.location().clone())
        .collect()
}

/// The folder pane `side` shows.
fn shown(state: &AppState, side: usize) -> VPath {
    state.panes()[side].active_tab().location().clone()
}

/// The folders `effects` list, in order.
fn listed(effects: &[Effect]) -> Vec<VPath> {
    loads(effects)
        .iter()
        .map(|request| request.path.clone())
        .collect()
}

/// The listing of request `request` comes with `entries`.
fn list(state: &mut AppState, request: &LoadRequest, entries: Vec<Entry>) {
    let _ = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries,
    });
}

fn share_is(state: &AppState, share: f32) -> bool {
    (state.layout().share() - share).abs() < 1e-6
}

#[test]
fn the_window_starts_with_two_panes_side_by_side_split_evenly_and_the_tree() {
    let state = ready(vec![], vec![]);
    let layout = state.layout();
    assert_eq!(layout, Layout::default());
    assert!(layout.dual && !layout.stacked && layout.tree && !layout.preview);
    assert!(share_is(&state, 0.5));
    assert!(!state.syncing());
}

#[test]
fn one_pane_shows_the_active_one_and_tab_stays_on_it() {
    let mut state = ready(vec![file("x")], vec![file("z")]);
    press(&mut state, &special(Key::Tab), NONE);
    assert_eq!(state.active(), RIGHT);

    assert_eq!(press(&mut state, &special(Key::F12), NONE), Some(vec![]));
    let layout = state.layout();
    assert!(!layout.dual);
    assert!(layout.shows(RIGHT, state.active()));
    assert!(!layout.shows(LEFT, state.active()));
    // Tab does not go to the hidden pane, and the key goes no further either.
    assert_eq!(press(&mut state, &special(Key::Tab), NONE), Some(vec![]));
    assert_eq!(press(&mut state, &special(Key::Tab), SHIFT), Some(vec![]));
    assert_eq!(state.active(), RIGHT);

    // Two panes again: both show, and Tab goes across.
    press(&mut state, &special(Key::F12), NONE);
    assert!(state.layout().dual);
    assert!(state.layout().shows(LEFT, state.active()));
    press(&mut state, &special(Key::Tab), NONE);
    assert_eq!(state.active(), LEFT);
}

#[test]
fn in_one_pane_the_picker_offers_the_hidden_pane() {
    let mut state = ready(vec![file("x.txt")], vec![dir("백업")]);
    press(&mut state, &special(Key::F12), NONE);
    press(&mut state, "a", CTRL);

    press(&mut state, &special(Key::F7), NONE);

    let Some(Modal::Pick(picker)) = state.modal() else {
        panic!("the picker shows: {:?}", state.modal());
    };
    let opposite = &picker.groups[0];
    assert_eq!(opposite.title, words::pick_opposite());
    assert_eq!(opposite.items[0].path, folder("b"));
    assert_eq!(picker.target, Some(folder("b")));
}

#[test]
fn the_split_turns_and_the_divider_keeps_each_pane_a_tenth_at_least() {
    let mut state = ready(vec![], vec![]);
    assert_eq!(press(&mut state, &special(Key::F12), SHIFT), Some(vec![]));
    assert!(state.layout().stacked);

    for (dragged, kept) in [
        (0.3, 0.3),
        (0.05, 0.1),
        (0.95, 0.9),
        (-3.0, 0.1),
        (f64::INFINITY, 0.9),
        (0.3334, 0.333),
    ] {
        state.set_split_share(dragged);
        assert!(
            share_is(&state, kept),
            "{dragged}: {}",
            state.layout().share()
        );
    }
    // A share that is no number changes nothing.
    state.set_split_share(f64::NAN);
    assert!(share_is(&state, 0.333));

    // Side by side again, with the same share.
    press(&mut state, &special(Key::F12), SHIFT);
    assert!(!state.layout().stacked);
    assert!(share_is(&state, 0.333));
    // A double click on the divider, or the 보기 menu, shares evenly.
    state.set_split_share(EVEN_SHARE);
    assert!(share_is(&state, 0.5));
}

#[test]
fn a_hidden_tree_stops_following_and_catches_up_when_it_shows() {
    let mut state = ready(vec![], vec![]);
    let mut effects = state.set_places(&[drive("임시", &temp())]);
    effects.extend(state.follow_active());
    let request = the_tree_load(&effects);
    list(&mut state, &request, vec![dir("a"), dir("b")]);
    assert_eq!(state.tree().selected_row(), Some(1));

    // Hidden, the tree no longer follows the active pane (and has no keys to keep).
    assert_eq!(press(&mut state, "b", CTRL), Some(vec![Effect::FocusPanes]));
    assert!(!state.layout().tree);
    press(&mut state, &special(Key::Tab), NONE);
    assert_eq!(state.follow_active(), []);
    assert_eq!(state.tree().selected_row(), Some(1));

    // Shown again, it selects the folder the active pane shows.
    press(&mut state, "b", CTRL);
    assert!(state.layout().tree);
    assert_eq!(state.follow_active(), [Effect::ScrollTree(2)]);
}

#[test]
fn the_preview_panel_comes_and_goes() {
    let mut state = ready(vec![], vec![]);
    assert_eq!(press(&mut state, "p", ALT), Some(vec![]));
    assert!(state.layout().preview);
    press(&mut state, "p", ALT);
    assert!(!state.layout().preview);
}

#[test]
fn swapping_trades_the_tab_sets_with_their_history() {
    let mut state = ready(vec![dir("x")], vec![file("z")]);
    // The left pane gets a second tab, which goes into x.
    let second = the_load(&press(&mut state, "t", CTRL).expect("a shortcut"));
    list(&mut state, &second, vec![dir("x")]);
    let into_x = the_load(&state.open_row(LEFT, 0));
    assert_eq!(into_x.path, in_a("x"));

    assert_eq!(press(&mut state, "u", CTRL), Some(vec![]));

    assert_eq!(locations(&state, LEFT), [folder("b")]);
    assert_eq!(rows(&state, LEFT), ["z"]);
    assert_eq!(locations(&state, RIGHT), [folder("a"), in_a("x")]);
    assert_eq!(state.panes()[RIGHT].active(), 1);
    assert!(state.panes()[RIGHT].active_tab().history().can_back());
    // The keyboard stays on its side, which shows the other tabs now.
    assert_eq!(state.active(), LEFT);
    // The moved tab's listing still reaches it.
    list(&mut state, &into_x, vec![file("inner")]);
    assert_eq!(rows(&state, RIGHT), ["inner"]);

    // Swapped back, each pane has its own tabs again.
    press(&mut state, "u", CTRL);
    assert_eq!(locations(&state, LEFT), [folder("a"), in_a("x")]);
    assert_eq!(locations(&state, RIGHT), [folder("b")]);
}

#[test]
fn in_one_pane_swapping_shows_the_hidden_tabs() {
    let mut state = ready(vec![file("x")], vec![file("z")]);
    press(&mut state, &special(Key::F12), NONE);
    press(&mut state, "u", CTRL);
    assert_eq!(state.active(), LEFT);
    assert!(state.layout().shows(LEFT, state.active()));
    assert_eq!(rows(&state, LEFT), ["z"]);
}

#[test]
fn the_other_pane_opens_the_active_folder() {
    let mut state = ready(vec![dir("x")], vec![file("z")]);

    let effects = press(&mut state, &special(Key::F9), NONE).expect("a shortcut");

    assert_eq!(the_load(&effects).path, folder("a"));
    assert_eq!(shown(&state, RIGHT), folder("a"));
    assert!(state.panes()[RIGHT].active_tab().history().can_back());
    assert_eq!(state.active(), LEFT);
    assert_eq!(locations(&state, RIGHT).len(), 1, "in the tab it shows");

    // From the right pane, the left one follows it.
    press(&mut state, &special(Key::Tab), NONE);
    state.go_to(folder("c"));
    let effects = press(&mut state, &special(Key::F9), NONE).expect("a shortcut");
    assert_eq!(the_load(&effects).path, folder("c"));
    assert_eq!(shown(&state, LEFT), folder("c"));
}

/// A window whose panes, at `a` and `b`, both hold the folder `x`; the left one also holds the
/// folder `y`, the right one has been into nothing yet. Sync is on.
fn synced() -> AppState {
    let mut state = ready(
        vec![dir("x"), dir("y"), file("f")],
        vec![dir("x"), file("g")],
    );
    let effects = press(&mut state, &special(Key::F9), SHIFT).expect("a shortcut");
    assert_eq!(effects, [Effect::Notice(words::sync_on().to_owned())]);
    assert!(state.syncing());
    state
}

#[test]
fn sync_takes_the_other_pane_into_the_folder_of_the_same_name_and_back() {
    let mut state = synced();

    // Enter on x, the first row of the left pane: both panes go into x.
    let effects = press(&mut state, &special(Key::Return), NONE).expect("a shortcut");
    assert_eq!(listed(&effects), [in_a("x"), in_b("x")]);
    // Up, Back and Forward: both make the same move.
    let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&effects), [folder("a"), folder("b")]);
    let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&effects), [in_a("x"), in_b("x")]);
    let effects = press(&mut state, &special(Key::RightArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&effects), [folder("a"), folder("b")]);
    assert!(state.syncing());
    let requests: Vec<LoadRequest> = loads(&effects).into_iter().cloned().collect();
    list(
        &mut state,
        &requests[0],
        vec![dir("x"), dir("y"), file("f")],
    );
    list(&mut state, &requests[1], vec![dir("x"), file("g")]);

    // It works from either pane: a double click on x in the right one.
    let effects = state.open_row(RIGHT, 0);
    assert_eq!(listed(&effects), [in_b("x"), in_a("x")]);
    assert_eq!(state.active(), RIGHT);
    assert!(state.syncing());

    // Shift+F9 again turns it off: the other pane stays where it is.
    let effects = press(&mut state, &special(Key::F9), SHIFT).expect("a shortcut");
    assert_eq!(effects, [Effect::Notice(words::sync_off().to_owned())]);
    let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&effects), [folder("b")]);
}

#[test]
fn sync_turns_off_when_the_other_pane_has_no_folder_of_that_name() {
    let mut state = synced();

    let effects = state.open_row(LEFT, 1);

    assert_eq!(the_load(&effects).path, in_a("y"));
    assert!(effects.contains(&Effect::Notice(words::sync_no_folder("y"))));
    assert!(!state.syncing());
    assert_eq!(shown(&state, RIGHT), folder("b"));
    // Off, Up moves the left pane alone.
    let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&effects), [folder("a")]);
}

#[test]
fn sync_turns_off_when_the_other_pane_cannot_make_the_same_move() {
    // The left pane has been into x before sync is on: the right one has nothing to go back to.
    let mut state = ready(vec![dir("x")], vec![dir("x")]);
    let _ = state.open_row(LEFT, 0);
    press(&mut state, &special(Key::F9), SHIFT);

    let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("a shortcut");

    assert_eq!(the_load(&effects).path, folder("a"));
    assert!(effects.contains(&Effect::Notice(words::sync_cannot_follow().to_owned())));
    assert!(!state.syncing());

    // The right pane at a root cannot go up.
    let root = ancestors(&folder("b")).remove(0);
    let _ = state.enter_path(RIGHT, &root.to_uri());
    state.activate(LEFT);
    press(&mut state, &special(Key::F9), SHIFT);
    let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&effects), [folder("a").parent().expect("a parent")]);
    assert!(effects.contains(&Effect::Notice(words::sync_cannot_follow().to_owned())));
    assert!(!state.syncing());
}

#[test]
fn sync_leaves_the_other_pane_alone_for_other_moves() {
    let mut state = synced();
    // A piece of the path row, a typed path, the tree: the active pane moves alone.
    let effects = state.open_crumb(LEFT, 0);
    assert_eq!(loads(&effects).len(), 1);
    let effects = state.go_to(folder("c"));
    assert_eq!(listed(&effects), [folder("c")]);
    assert_eq!(shown(&state, RIGHT), folder("b"));
    // A move that does not happen (nothing to go forward to) asks nothing of the other pane.
    assert_eq!(
        press(&mut state, &special(Key::RightArrow), ALT),
        Some(vec![])
    );
    assert!(state.syncing());
}

#[test]
fn the_layout_comes_from_the_session_and_goes_back_into_it() {
    let mut layout = Layout::default();
    layout.dual = false;
    layout.stacked = true;
    layout.tree = false;
    layout.preview = true;
    layout.set_share(0.3);
    let session = Session {
        layout,
        ..Session::default()
    };

    let (mut state, _) = AppState::restore(folder("a"), folder("b"), session, &[], None);

    assert_eq!(state.layout(), layout);
    assert_eq!(state.session().layout, layout);
    // Sync is no part of it: it starts off.
    assert!(!state.syncing());
    press(&mut state, &special(Key::F9), SHIFT);
    assert_eq!(state.session().layout, layout);
}
