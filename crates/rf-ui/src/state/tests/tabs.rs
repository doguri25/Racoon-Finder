//! The tabs (NAV-01): duplicating, reopening, reordering, pinning and moving them.

use super::*;
use crate::settings::{Session, SessionPane};

fn locations(state: &AppState, side: usize) -> Vec<VPath> {
    state.panes()[side]
        .tabs()
        .iter()
        .map(|tab| tab.location().clone())
        .collect()
}

fn folders(names: &[&str]) -> Vec<VPath> {
    names.iter().map(|name| folder(name)).collect()
}

fn into_x() -> VPath {
    folder("a").join("x").expect("a plain name")
}

/// A window whose left pane has a tab at each folder of `names`, tab `active` showing and the
/// keyboard on that pane, and whose right pane is at `b`. Every tab has listed the folder `x`.
fn tabs_at(names: &[&str], active: usize) -> AppState {
    let session = Session {
        panes: [
            SessionPane {
                tabs: folders(names),
                active,
                pinned: Vec::new(),
            },
            SessionPane {
                tabs: vec![folder("b")],
                active: 0,
                pinned: Vec::new(),
            },
        ],
        ..Session::default()
    };
    let (mut state, effects) =
        AppState::restore(folder("a"), folder("b"), session, &[], Some(folder("home")));
    for request in loads(&effects) {
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![dir("x")],
        });
    }
    state
}

fn listed(state: &mut AppState, generation: u64, names: &[&str]) -> Vec<Effect> {
    state.loaded(LoadEvent::Listed {
        generation,
        entries: names.iter().map(|name| file(name)).collect(),
    })
}

fn back(state: &mut AppState) -> Vec<Effect> {
    press(state, &special(Key::LeftArrow), ALT).expect("the key is used")
}

fn shown(state: &AppState, side: usize) -> usize {
    state.panes()[side].active()
}

#[test]
fn a_duplicate_opens_beside_its_tab_with_the_same_folder_and_history() {
    let mut state = tabs_at(&["a", "c"], 0);
    // The first tab goes into x, so that it has a folder to go back to.
    let into = the_load(&state.open_row(LEFT, 0));
    listed(&mut state, into.generation, &["f"]);
    state.toggle_pin(LEFT, 0);

    let effects = state.duplicate_tab(LEFT, 0);

    let request = the_load(&effects);
    assert_eq!(request.path, into_x());
    assert_ne!(request.generation, into.generation);
    assert_eq!(locations(&state, LEFT), [into_x(), into_x(), folder("c")]);
    assert_eq!(shown(&state, LEFT), 1);
    let copy = &state.panes()[LEFT].tabs()[1];
    assert!(copy.history().can_back());
    assert!(!copy.pinned(), "a copy of a pinned tab is not pinned");
    assert_ne!(copy.id(), state.panes()[LEFT].tabs()[0].id());
    // Each tab takes its own listing.
    listed(&mut state, request.generation, &["g"]);
    assert_eq!(rows(&state, LEFT), ["g"]);
    state.select_tab(LEFT, 0);
    assert_eq!(rows(&state, LEFT), ["f"]);
    // Back in the copy goes where the tab came from.
    state.select_tab(LEFT, 1);
    assert_eq!(the_load(&back(&mut state)).path, folder("a"));
}

#[test]
fn a_reopened_tab_lists_its_folder_again_under_a_new_generation() {
    let mut state = tabs_at(&["a", "c"], 0);
    // The first tab goes into x and is closed while x is still being listed.
    let into = the_load(&state.open_row(LEFT, 0));
    press(&mut state, "w", CTRL);
    assert_eq!(locations(&state, LEFT), [folder("c")]);
    assert!(!state.generations().contains(&into.generation));
    assert!(state.panes()[LEFT].can_reopen());

    let effects = press(&mut state, "T", CTRL_SHIFT).expect("the key is used");

    // It comes back where it was, at its folder, which it lists again.
    let request = the_load(&effects);
    assert_eq!(request.path, into_x());
    assert!(request.generation > into.generation);
    assert_eq!(locations(&state, LEFT), [into_x(), folder("c")]);
    assert_eq!(shown(&state, LEFT), 0);
    assert!(state.generations().contains(&request.generation));
    assert!(!state.generations().contains(&into.generation));
    // The listing of the tab that was closed, late, does not fill it.
    assert_eq!(listed(&mut state, into.generation, &["late"]), []);
    assert_eq!(rows(&state, LEFT), Vec::<&str>::new());
    assert_eq!(
        state.panes()[LEFT].active_tab().status(),
        LoadStatus::Loading
    );
    listed(&mut state, request.generation, &["now"]);
    assert_eq!(rows(&state, LEFT), ["now"]);
    // Its history came back with it.
    assert_eq!(the_load(&back(&mut state)).path, folder("a"));
    // Nothing more to reopen: the key does nothing.
    assert!(!state.panes()[LEFT].can_reopen());
    assert_eq!(press(&mut state, "T", CTRL_SHIFT), Some(Vec::new()));
}

#[test]
fn closing_others_or_those_to_the_right_spares_pinned_tabs_and_reopens_them_in_place() {
    let names = ["t0", "t1", "t2", "t3", "t4"];
    let mut state = tabs_at(&names, 4);
    state.toggle_pin(LEFT, 3);

    // To the right of t1: t2 and t4 go, the pinned t3 stays, and t1 shows (t4 showed).
    state.close_tabs_right(LEFT, 1);
    assert_eq!(locations(&state, LEFT), folders(&["t0", "t1", "t3"]));
    assert_eq!(shown(&state, LEFT), 1);
    // The others of t1: t0 goes.
    state.select_tab(LEFT, 2);
    state.close_other_tabs(LEFT, 1);
    assert_eq!(locations(&state, LEFT), folders(&["t1", "t3"]));
    assert_eq!(shown(&state, LEFT), 0);

    // Reopened one by one, they come back to their places.
    for _ in 0..3 {
        assert_eq!(loads(&state.reopen_tab(LEFT)).len(), 1);
    }
    assert_eq!(locations(&state, LEFT), folders(&names));
    let pinned: Vec<bool> = state.panes()[LEFT].tabs().iter().map(Tab::pinned).collect();
    assert_eq!(pinned, [false, false, false, true, false]);
    assert_eq!(state.reopen_tab(LEFT), []);
    // Ctrl+W closes a pinned tab: it was asked for by name.
    state.select_tab(LEFT, 3);
    press(&mut state, "w", CTRL);
    assert_eq!(locations(&state, LEFT), folders(&["t0", "t1", "t2", "t4"]));
}

#[test]
fn a_pane_remembers_the_last_ten_tabs_it_closed() {
    let names: Vec<String> = (0..12).map(|i| format!("t{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut state = tabs_at(&names, 0);
    for _ in 0..11 {
        state.close_tab(LEFT, 0);
    }
    assert_eq!(locations(&state, LEFT), folders(&["t11"]));
    // The other pane has none of them.
    assert_eq!(state.reopen_tab(RIGHT), []);

    for _ in 0..12 {
        state.reopen_tab(LEFT);
    }

    assert_eq!(locations(&state, LEFT), folders(&names[1..]));
}

#[test]
fn a_pinned_tab_stays_and_opens_other_folders_in_a_new_tab_beside_it() {
    let mut state = tabs_at(&["a", "c"], 0);
    state.toggle_pin(LEFT, 0);
    assert!(state.panes()[LEFT].tabs()[0].pinned());

    // Opening a folder of its list.
    let request = the_load(&state.open_row(LEFT, 0));
    assert_eq!(request.path, into_x());
    assert_eq!(
        locations(&state, LEFT),
        [folder("a"), into_x(), folder("c")]
    );
    assert_eq!(shown(&state, LEFT), 1);
    let tabs = state.panes()[LEFT].tabs();
    assert!(!tabs[1].pinned());
    assert_eq!(
        tabs[0].status(),
        LoadStatus::Ready,
        "the pinned tab keeps its rows"
    );
    // Back in the new tab goes to the pinned tab's folder, in the new tab.
    assert_eq!(the_load(&back(&mut state)).path, folder("a"));
    assert_eq!(locations(&state, LEFT).len(), 3);

    // Up, a piece of the path row, a typed path, the tree: each opens a tab beside it.
    let parent = folder("a").parent().expect("the temp folder");
    state.select_tab(LEFT, 0);
    let up = press(&mut state, &special(Key::UpArrow), ALT).expect("the key is used");
    assert_eq!(the_load(&up).path, parent);
    state.select_tab(LEFT, 0);
    let root = ancestors(&folder("a")).remove(0);
    assert_eq!(the_load(&state.open_crumb(LEFT, 0)).path, root);
    state.select_tab(LEFT, 0);
    let typed = std::env::temp_dir().join("typed");
    let effects = state.enter_path(LEFT, &typed.display().to_string());
    assert_eq!(the_load(&effects).path, folder("typed"));
    state.select_tab(LEFT, 0);
    assert_eq!(the_load(&state.go_to(folder("tree"))).path, folder("tree"));
    assert_eq!(
        locations(&state, LEFT),
        [
            folder("a"),
            folder("tree"),
            folder("typed"),
            root,
            parent,
            folder("a"),
            folder("c")
        ]
    );
    assert!(state.panes()[LEFT].tabs()[0].pinned());

    // Its own folder again lists it again, in the pinned tab itself.
    state.select_tab(LEFT, 0);
    let again = the_load(&state.go_to(folder("a")));
    assert_eq!(again.generation, state.panes()[LEFT].tabs()[0].generation());
    assert_eq!(locations(&state, LEFT).len(), 7);
    // A move it cannot make opens no tab.
    assert_eq!(back(&mut state), []);
    assert_eq!(locations(&state, LEFT).len(), 7);

    // Unpinned, it moves itself again.
    state.toggle_pin(LEFT, 0);
    let _ = state.open_row(LEFT, 0);
    assert_eq!(locations(&state, LEFT)[0], into_x());
    assert_eq!(locations(&state, LEFT).len(), 7);
}

#[test]
fn the_other_pane_keeps_its_pinned_tab_too() {
    let mut state = ready(vec![dir("x")], vec![dir("x")]);
    state.toggle_pin(RIGHT, 0);
    state.activate(LEFT);

    // The same folder in the other pane (F9) opens there in a new tab.
    let effects = press(&mut state, &special(Key::F9), NONE).expect("the key is used");

    assert_eq!(the_load(&effects).path, folder("a"));
    assert_eq!(locations(&state, RIGHT), [folder("b"), folder("a")]);
    assert_eq!(shown(&state, RIGHT), 1);
    assert_eq!(state.active(), LEFT);
}

#[test]
fn tabs_move_along_the_strip_by_drag_and_by_keys() {
    let mut state = tabs_at(&["t0", "t1", "t2", "t3"], 1);
    let id = state.panes()[LEFT].tabs()[1].id();
    let generation = state.panes()[LEFT].tabs()[1].generation();

    // A drag of t1 to the end of the strip.
    assert!(state.tab_drop_offer(id, LEFT, 4));
    assert_eq!(state.drop_tab(id, LEFT, 4), []);
    assert_eq!(locations(&state, LEFT), folders(&["t0", "t2", "t3", "t1"]));
    assert_eq!(shown(&state, LEFT), 3, "the tab that showed still shows");
    assert_eq!(state.panes()[LEFT].tabs()[3].generation(), generation);
    // The places on either side of it would leave it where it is: they take no drop.
    assert!(!state.tab_drop_offer(id, LEFT, 3));
    assert!(!state.tab_drop_offer(id, LEFT, 4));
    // Nor does a drag of no tab, or one that went (its id is no tab's).
    assert!(!state.tab_drop_offer(0, LEFT, 0));
    assert_eq!(state.drop_tab(u64::MAX, LEFT, 0), []);

    // Ctrl+Shift+PageUp and PageDown move the tab that shows, as far as the ends.
    let left = special(Key::PageUp);
    let right = special(Key::PageDown);
    press(&mut state, &left, CTRL_SHIFT);
    assert_eq!(locations(&state, LEFT), folders(&["t0", "t2", "t1", "t3"]));
    assert_eq!(shown(&state, LEFT), 2);
    for _ in 0..3 {
        press(&mut state, &left, CTRL_SHIFT);
    }
    assert_eq!(locations(&state, LEFT), folders(&["t1", "t0", "t2", "t3"]));
    assert_eq!(shown(&state, LEFT), 0);
    press(&mut state, &right, CTRL_SHIFT);
    assert_eq!(locations(&state, LEFT), folders(&["t0", "t1", "t2", "t3"]));
    assert_eq!(shown(&state, LEFT), 1);
}

#[test]
fn a_tab_moved_to_the_other_pane_takes_its_listing_along() {
    let mut state = ready(vec![dir("x")], vec![]);
    let request = the_load(&state.add_tab(LEFT));
    state.toggle_pin(LEFT, 1);

    // From the tab's menu: it shows in the other pane, which gets the keyboard.
    assert_eq!(state.move_tab_to_other(LEFT, 1), []);
    assert_eq!(locations(&state, LEFT), [folder("a")]);
    assert_eq!(locations(&state, RIGHT), [folder("b"), folder("a")]);
    assert_eq!(shown(&state, RIGHT), 1);
    assert_eq!(state.active(), RIGHT);
    assert!(state.panes()[RIGHT].tabs()[1].pinned());
    assert!(state.generations().contains(&request.generation));
    // Its listing, late, reaches it in its new pane.
    listed(&mut state, request.generation, &["moved"]);
    assert_eq!(rows(&state, RIGHT), ["moved"]);
    assert_eq!(rows(&state, LEFT), ["x"]);

    // A pane's last tab stays.
    assert_eq!(
        state.move_tab_to_other(LEFT, 0),
        [Effect::Notice(words::last_tab_stays().to_owned())]
    );
    assert!(!state.tab_drop_offer(state.panes()[LEFT].tabs()[0].id(), RIGHT, 0));

    // A drag to the start of the other pane's strip.
    let id = state.panes()[RIGHT].tabs()[1].id();
    assert!(state.tab_drop_offer(id, LEFT, 0));
    assert_eq!(state.drop_tab(id, LEFT, 0), []);
    assert_eq!(locations(&state, LEFT), [folder("a"), folder("a")]);
    assert_eq!(rows(&state, LEFT), ["moved"]);
    assert_eq!(shown(&state, LEFT), 0);
    assert_eq!(state.active(), LEFT);
    assert_eq!(locations(&state, RIGHT), [folder("b")]);
}

#[test]
fn with_one_pane_a_tab_moves_to_the_hidden_pane_and_a_notice_says_so() {
    let mut state = ready(vec![], vec![]);
    let _ = state.add_tab(LEFT);
    press(&mut state, &special(Key::F12), NONE);
    assert!(!state.layout().dual);

    let effects = state.move_tab_to_other(LEFT, 1);

    assert_eq!(
        effects,
        [Effect::Notice(words::tab_moved_to_hidden().to_owned())]
    );
    assert_eq!(state.active(), LEFT, "the pane that shows keeps the keys");
    assert_eq!(locations(&state, LEFT), [folder("a")]);
    assert_eq!(locations(&state, RIGHT), [folder("b"), folder("a")]);
    // Nor can a drag reach the hidden pane's strip.
    let _ = state.add_tab(LEFT);
    let id = state.panes()[LEFT].tabs()[0].id();
    assert!(!state.tab_drop_offer(id, RIGHT, 0));
}

#[test]
fn a_tab_moved_away_no_longer_follows_navigation_sync() {
    // The right pane's tab is still listing when the left one goes into x: sync waits.
    let mut state = ready(vec![dir("x")], vec![dir("x")]);
    let _ = state.add_tab(RIGHT);
    let waiting = state.panes()[RIGHT].active_tab().generation();
    state.activate(LEFT);
    press(&mut state, &special(Key::F9), SHIFT);
    assert!(state.syncing());
    let _ = state.open_row(LEFT, 0);

    // That tab goes to the left pane before its listing comes.
    let _ = state.move_tab_to_other(RIGHT, 1);
    let effects = state.loaded(LoadEvent::Listed {
        generation: waiting,
        entries: vec![dir("x")],
    });

    // It does not go into x for a sync of the pane it left, nor does the right pane.
    assert_eq!(loads(&effects), Vec::<&LoadRequest>::new());
    assert!(state.syncing());
    assert_eq!(locations(&state, RIGHT), [folder("b")]);
}

#[test]
fn pinned_tabs_and_their_order_are_kept_for_the_next_run() {
    let session = Session {
        panes: [
            SessionPane {
                tabs: folders(&["a", "c", "d"]),
                active: 2,
                pinned: vec![false, true],
            },
            SessionPane {
                tabs: folders(&["b"]),
                active: 0,
                pinned: vec![true],
            },
        ],
        ..Session::default()
    };
    let (mut state, _) = AppState::restore(folder("a"), folder("b"), session.clone(), &[], None);
    let pinned = |state: &AppState, side: usize| -> Vec<bool> {
        state.panes()[side].tabs().iter().map(Tab::pinned).collect()
    };
    assert_eq!(pinned(&state, LEFT), [false, true, false]);
    assert_eq!(pinned(&state, RIGHT), [true]);
    assert_eq!(state.session(), session);

    // Moved and unpinned, the tabs are kept as they are now.
    let id = state.panes()[LEFT].tabs()[2].id();
    state.drop_tab(id, LEFT, 0);
    state.toggle_pin(RIGHT, 0);
    let now = state.session();
    assert_eq!(now.panes[LEFT].tabs, folders(&["d", "a", "c"]));
    assert_eq!(now.panes[LEFT].pinned, [false, false, true]);
    assert_eq!(now.panes[LEFT].active, 0);
    assert_eq!(now.panes[RIGHT].pinned, Vec::<bool>::new());
}
