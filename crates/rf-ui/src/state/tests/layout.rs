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

fn in_c(name: &str) -> VPath {
    folder("c").join(name).expect("a plain name")
}

/// A window whose left pane at `a` holds the folders x and y, and whose right pane has just
/// gone to `c`, whose listing (the returned request) has not come yet. Sync is on.
fn synced_waiting() -> (AppState, LoadRequest) {
    let mut state = ready(vec![dir("x"), dir("y")], vec![file("g")]);
    state.activate(RIGHT);
    let waiting = the_load(&state.go_to(folder("c")));
    state.activate(LEFT);
    press(&mut state, &special(Key::F9), SHIFT);
    assert!(state.syncing());
    (state, waiting)
}

fn notices(effects: &[Effect]) -> Vec<&str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notice(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn sync_waits_for_the_other_pane_s_listing_before_it_decides() {
    let (mut state, waiting) = synced_waiting();

    // Enter on x: the left pane goes into it; the right one has no rows to look in yet.
    let effects = state.open_row(LEFT, 0);
    assert_eq!(listed(&effects), [in_a("x")]);
    assert_eq!(notices(&effects), Vec::<&str>::new());
    assert!(state.syncing());

    // Its listing comes with x: the right pane goes into it.
    let effects = state.loaded(LoadEvent::Listed {
        generation: waiting.generation,
        entries: vec![dir("x"), file("h")],
    });
    assert_eq!(listed(&effects), [in_c("x")]);
    assert_eq!(shown(&state, RIGHT), in_c("x"));
    assert!(state.syncing());
}

#[test]
fn folders_entered_while_the_other_pane_lists_are_entered_there_in_turn() {
    let (mut state, waiting) = synced_waiting();
    let into_x = the_load(&state.open_row(LEFT, 0));
    list(&mut state, &into_x, vec![dir("y")]);
    let into_y = state.open_row(LEFT, 0);
    assert_eq!(listed(&into_y), [in_a("x").join("y").expect("a name")]);

    let effects = state.loaded(LoadEvent::Listed {
        generation: waiting.generation,
        entries: vec![dir("x")],
    });
    let next = the_load(&effects);
    assert_eq!(next.path, in_c("x"));
    let effects = state.loaded(LoadEvent::Listed {
        generation: next.generation,
        entries: vec![dir("y")],
    });
    assert_eq!(listed(&effects), [in_c("x").join("y").expect("a name")]);
    assert!(state.syncing());
}

#[test]
fn sync_turns_off_when_the_listing_it_waited_for_lacks_the_folder() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);

    let effects = state.loaded(LoadEvent::Listed {
        generation: waiting.generation,
        entries: vec![dir("other")],
    });

    assert_eq!(notices(&effects), [words::sync_no_folder("x")]);
    assert!(!state.syncing());
    assert_eq!(shown(&state, RIGHT), folder("c"));

    // A listing that fails cannot be followed either.
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    let effects = state.loaded(LoadEvent::Failed {
        generation: waiting.generation,
        kind: ErrorKind::NotFound,
    });
    assert_eq!(notices(&effects), [words::sync_cannot_follow()]);
    assert!(!state.syncing());
}

#[test]
fn a_move_of_the_other_pane_drops_the_decision_sync_waited_to_make() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);

    // The user takes the right pane to its root before its listing comes.
    let root = ancestors(&folder("c")).remove(0);
    let moved = the_load(&state.open_crumb(RIGHT, 0));
    assert_eq!(moved.path, root);

    // The old listing is late, and the new one is no answer: nothing is entered.
    assert_eq!(
        state.loaded(LoadEvent::Listed {
            generation: waiting.generation,
            entries: vec![dir("x")],
        }),
        []
    );
    let effects = state.loaded(LoadEvent::Listed {
        generation: moved.generation,
        entries: vec![dir("x")],
    });
    assert_eq!(listed(&effects), Vec::<VPath>::new());
    assert_eq!(notices(&effects), Vec::<&str>::new());
    assert_eq!(shown(&state, RIGHT), root);
    assert!(state.syncing());
}

#[test]
fn back_while_the_other_pane_waits_undoes_the_entry_there_too() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);

    let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("a shortcut");

    assert_eq!(listed(&effects), [folder("a")]);
    assert_eq!(notices(&effects), Vec::<&str>::new());
    assert_eq!(
        state.loaded(LoadEvent::Listed {
            generation: waiting.generation,
            entries: vec![dir("x")],
        }),
        []
    );
    assert_eq!(shown(&state, RIGHT), folder("c"));
    assert!(state.syncing());
}

#[test]
fn up_while_the_other_pane_waits_turns_sync_off() {
    // Up leaves x for a as Back would, but as a move of its own: the left pane's Back goes to x
    // from there, where the right pane, which never went into x, cannot follow.
    for listing_first in [false, true] {
        let (mut state, waiting) = synced_waiting();
        let _ = state.open_row(LEFT, 0);

        let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("a shortcut");

        assert_eq!(listed(&effects), [folder("a")]);
        assert_eq!(notices(&effects), [words::sync_cannot_follow()]);
        assert!(!state.syncing());
        let listing = || LoadEvent::Listed {
            generation: waiting.generation,
            entries: vec![dir("x")],
        };
        if listing_first {
            assert_eq!(state.loaded(listing()), []);
            assert_eq!(shown(&state, RIGHT), folder("c"));
        }
        // Back moves the left pane alone.
        let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("a shortcut");
        assert_eq!(
            listed(&effects),
            [in_a("x")],
            "listing first: {listing_first}"
        );
        if !listing_first {
            assert_eq!(state.loaded(listing()), []);
        }
        assert_eq!(shown(&state, RIGHT), folder("c"));
        assert!(!state.syncing());
    }
}

#[test]
fn a_vanished_folder_sync_waits_in_turns_it_off_with_the_same_notice() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    let parent = folder("c").parent().expect("the temp folder");
    let _ = state.loaded(LoadEvent::Vanished {
        generation: waiting.generation,
    });

    // c is gone: the right pane goes to its parent, and cannot open x as it waited to.
    let effects = state.found_existing(
        waiting.generation,
        &folder("c"),
        Some(parent.clone()),
        Instant::now(),
    );

    assert_eq!(listed(&effects), std::slice::from_ref(&parent));
    assert_eq!(
        notices(&effects),
        [words::folder_vanished_sync_off("c", true)]
    );
    assert!(!state.syncing());

    // A wait that kept only the folders Back took off goes quietly: sync stays on.
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    let _ = press(&mut state, &special(Key::LeftArrow), ALT);
    let _ = state.loaded(LoadEvent::Vanished {
        generation: waiting.generation,
    });
    let effects = state.found_existing(
        waiting.generation,
        &folder("c"),
        Some(parent.clone()),
        Instant::now(),
    );
    assert_eq!(notices(&effects), [words::folder_vanished("c", true)]);
    assert!(state.syncing());
}

#[test]
fn turning_sync_off_drops_the_decision_it_waited_to_make() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    press(&mut state, &special(Key::F9), SHIFT);
    assert!(!state.syncing());
    press(&mut state, &special(Key::F9), SHIFT);

    let effects = state.loaded(LoadEvent::Listed {
        generation: waiting.generation,
        entries: vec![dir("x")],
    });

    assert_eq!(effects, []);
    assert_eq!(shown(&state, RIGHT), folder("c"));
}

#[test]
fn sync_takes_a_folder_that_differs_in_case_alone_where_the_system_ignores_case() {
    let mut state = ready(vec![dir("Docs")], vec![dir("docs"), dir("other")]);
    press(&mut state, &special(Key::F9), SHIFT);

    let effects = state.open_row(LEFT, 0);

    if Os::CURRENT == Os::Other {
        assert_eq!(listed(&effects), [in_a("Docs")]);
        assert!(!state.syncing());
    } else {
        assert_eq!(listed(&effects), [in_a("Docs"), in_b("docs")]);
        assert!(state.syncing());
    }
}

#[test]
fn the_folder_of_the_same_name_is_the_exact_one_or_the_only_one_in_another_case() {
    use super::super::layout::same_folder;
    let entries = vec![dir("Docs"), dir("docs"), dir("Music"), file("notes")];
    for os in [Os::Windows, Os::MacOs, Os::Other] {
        // The very name first, whatever the system.
        assert_eq!(
            same_folder(&entries, "docs".as_ref(), os),
            Some("docs".as_ref())
        );
        assert_eq!(
            same_folder(&entries, "Docs".as_ref(), os),
            Some("Docs".as_ref())
        );
        // A file is no folder.
        assert_eq!(same_folder(&entries, "notes".as_ref(), os), None);
    }
    for os in [Os::Windows, Os::MacOs] {
        assert_eq!(
            same_folder(&entries, "music".as_ref(), os),
            Some("Music".as_ref())
        );
        // Two that differ in case alone: neither is taken.
        assert_eq!(same_folder(&entries, "DOCS".as_ref(), os), None);
    }
    assert_eq!(same_folder(&entries, "music".as_ref(), Os::Other), None);
}

#[test]
fn with_one_pane_the_commands_for_the_hidden_one_only_say_so() {
    let mut state = ready(vec![dir("x")], vec![file("z")]);
    press(&mut state, &special(Key::F9), SHIFT);

    // One pane: sync goes off, as the other pane does not show.
    let effects = press(&mut state, &special(Key::F12), NONE).expect("a shortcut");
    assert_eq!(effects, [Effect::Notice(words::sync_off().to_owned())]);
    assert!(!state.syncing());

    let one_pane = Some(vec![Effect::Notice(words::one_pane_only().to_owned())]);
    assert_eq!(press(&mut state, &special(Key::F9), NONE), one_pane);
    assert_eq!(shown(&state, RIGHT), folder("b"));
    assert_eq!(press(&mut state, &special(Key::F9), SHIFT), one_pane);
    assert!(!state.syncing());
}

#[test]
fn back_then_forward_while_the_other_pane_waits_puts_the_entry_back() {
    // Both before the other pane's listing comes.
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    let back = press(&mut state, &special(Key::LeftArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&back), [folder("a")]);
    let forward = press(&mut state, &special(Key::RightArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&forward), [in_a("x")]);
    assert_eq!(notices(&forward), Vec::<&str>::new());
    let effects = state.loaded(LoadEvent::Listed {
        generation: waiting.generation,
        entries: vec![dir("x")],
    });
    assert_eq!(listed(&effects), [in_c("x")]);
    assert!(state.syncing());

    // Forward after the listing came: the other pane goes in at once.
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    let _ = press(&mut state, &special(Key::LeftArrow), ALT);
    let effects = state.loaded(LoadEvent::Listed {
        generation: waiting.generation,
        entries: vec![dir("x")],
    });
    assert_eq!(effects, []);
    assert_eq!(shown(&state, RIGHT), folder("c"));
    let forward = press(&mut state, &special(Key::RightArrow), ALT).expect("a shortcut");
    assert_eq!(listed(&forward), [in_a("x"), in_c("x")]);
    assert!(state.syncing());
}

#[test]
fn back_past_where_the_wait_began_turns_sync_off() {
    // The left pane went into y before: it can go back past the folder the wait began in,
    // where the other pane's history is not in step.
    let mut state = ready(vec![dir("x"), dir("y")], vec![file("g")]);
    let into_y = the_load(&state.open_row(LEFT, 1));
    list(&mut state, &into_y, vec![dir("z")]);
    state.activate(RIGHT);
    let waiting = the_load(&state.go_to(folder("c")));
    state.activate(LEFT);
    press(&mut state, &special(Key::F9), SHIFT);
    let _ = state.open_row(LEFT, 0);
    let _ = press(&mut state, &special(Key::LeftArrow), ALT);

    let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("a shortcut");

    assert_eq!(listed(&effects), [folder("a")]);
    assert_eq!(notices(&effects), [words::sync_cannot_follow()]);
    assert!(!state.syncing());
    assert_eq!(
        state.loaded(LoadEvent::Listed {
            generation: waiting.generation,
            entries: vec![dir("z")],
        }),
        []
    );
}

#[test]
fn a_wait_goes_on_through_a_reload_after_the_other_pane_s_watch_ended() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    let now = Instant::now();

    // Its watch ends before its listing comes; the folder is still there: it is listed again.
    let effects = state.loaded(LoadEvent::Vanished {
        generation: waiting.generation,
    });
    assert_eq!(
        effects,
        [Effect::FindExisting {
            generation: waiting.generation,
            path: folder("c"),
        }]
    );
    let again =
        the_load(&state.found_existing(waiting.generation, &folder("c"), Some(folder("c")), now));

    // Once more within the gap: the reload waits for it, and then lists it.
    let _ = state.loaded(LoadEvent::Vanished {
        generation: again.generation,
    });
    let later = state.found_existing(again.generation, &folder("c"), Some(folder("c")), now);
    assert!(
        matches!(later.as_slice(), [Effect::ReloadLater { .. }]),
        "{later:?}"
    );
    let last = the_load(&state.reload_due(again.generation, &folder("c"), now + AUTO_RELOAD_GAP));

    let effects = state.loaded(LoadEvent::Listed {
        generation: last.generation,
        entries: vec![dir("x")],
    });
    assert_eq!(listed(&effects), [in_c("x")]);
    assert!(state.syncing());
}

#[test]
fn a_wait_goes_on_through_f5_in_the_other_pane() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    // The other pane is listed again by hand before its listing comes: sync waits for the new
    // listing, and the old one's request is dropped.
    state.activate(RIGHT);
    let again = the_load(&press(&mut state, &special(Key::F5), NONE).expect("used"));
    assert_ne!(again.generation, waiting.generation);
    state.activate(LEFT);
    assert_eq!(
        state.loaded(LoadEvent::Listed {
            generation: waiting.generation,
            entries: vec![dir("x")],
        }),
        []
    );
    let effects = state.loaded(LoadEvent::Listed {
        generation: again.generation,
        entries: vec![dir("x")],
    });
    assert_eq!(listed(&effects), [in_c("x")]);
    assert!(state.syncing());
}

#[test]
fn a_wait_goes_on_through_a_retry_in_the_other_pane() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    // 다시 시도 lists it again as F5 does: sync waits for that listing.
    let again = the_load(&state.retry(RIGHT));
    state.activate(LEFT);
    let effects = state.loaded(LoadEvent::Listed {
        generation: again.generation,
        entries: vec![dir("x")],
    });
    assert_ne!(again.generation, waiting.generation);
    assert_eq!(listed(&effects), [in_c("x")]);
    assert!(state.syncing());
}

#[test]
fn swapping_the_panes_while_sync_waits_turns_it_off() {
    let (mut state, waiting) = synced_waiting();
    let _ = state.open_row(LEFT, 0);

    let effects = press(&mut state, "u", CTRL).expect("a shortcut");

    assert_eq!(
        effects,
        [Effect::Notice(words::sync_cannot_follow().to_owned())]
    );
    assert!(!state.syncing());
    // The tab that waited now shows on the left, the side with the keyboard: nothing enters it.
    assert_eq!(
        state.loaded(LoadEvent::Listed {
            generation: waiting.generation,
            entries: vec![dir("x")],
        }),
        []
    );
    assert_eq!(shown(&state, LEFT), folder("c"));
    // Without a wait, a swap leaves sync as it is; so it does with a wait for a tab that has
    // moved on since.
    let mut state = synced();
    assert_eq!(press(&mut state, "u", CTRL), Some(vec![]));
    assert!(state.syncing());
    let (mut state, _) = synced_waiting();
    let _ = state.open_row(LEFT, 0);
    let _ = state.open_crumb(RIGHT, 0);
    assert_eq!(press(&mut state, "u", CTRL), Some(vec![]));
    assert!(state.syncing());
}
