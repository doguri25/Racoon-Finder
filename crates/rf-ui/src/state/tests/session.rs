//! Opening the tabs of the last run, and handing them back for the settings.

use super::*;
use crate::settings::{Session, SessionPane, Waiting};

fn session(left: &[&str], left_active: usize, right: &[&str], active_pane: usize) -> Session {
    let pane = |names: &[&str], active| SessionPane {
        tabs: names.iter().map(|name| folder(name)).collect(),
        active,
    };
    Session {
        panes: [pane(left, left_active), pane(right, 0)],
        active_pane,
        show_hidden: true,
    }
}

fn locations(state: &AppState, side: usize) -> Vec<VPath> {
    state.panes()[side]
        .tabs()
        .iter()
        .map(|tab| tab.location().clone())
        .collect()
}

#[test]
fn a_session_opens_its_tabs() {
    let (state, effects) = AppState::restore(
        folder("default-left"),
        folder("default-right"),
        session(&["a", "b", "c"], 1, &["d"], 1),
        &[],
        Some(folder("home")),
    );

    assert_eq!(
        locations(&state, LEFT),
        [folder("a"), folder("b"), folder("c")]
    );
    assert_eq!(state.panes()[LEFT].active(), 1);
    assert_eq!(locations(&state, RIGHT), [folder("d")]);
    assert_eq!(state.active(), RIGHT);
    assert!(state.show_hidden());
    assert!(state.panes()[LEFT].tabs()[0].filter().show_hidden);
    // Every tab lists its folder.
    let listed: Vec<VPath> = loads(&effects).iter().map(|r| r.path.clone()).collect();
    assert_eq!(listed, [folder("a"), folder("b"), folder("c"), folder("d")]);

    assert_eq!(state.session(), session(&["a", "b", "c"], 1, &["d"], 1));
}

#[test]
fn a_pane_without_tabs_opens_its_default_folder() {
    let (state, _) = AppState::restore(
        folder("default-left"),
        folder("default-right"),
        session(&[], 0, &["d", "e"], 7),
        &[],
        None,
    );
    assert_eq!(locations(&state, LEFT), [folder("default-left")]);
    assert_eq!(locations(&state, RIGHT), [folder("d"), folder("e")]);
    assert_eq!(state.panes()[RIGHT].active(), 0);
    assert_eq!(
        state.active(),
        RIGHT,
        "a pane past the right one is the right one"
    );
}

#[test]
fn the_session_follows_the_tabs() {
    let mut state = ready(vec![dir("x")], vec![]);
    let _ = state.add_tab(RIGHT);
    let _ = state.go_to(folder("elsewhere"));
    let _ = state.select_tab(RIGHT, 0);
    let _ = press(&mut state, "h", CTRL);

    let now = state.session();

    assert_eq!(now.panes[LEFT].tabs, [folder("a")]);
    assert_eq!(now.panes[RIGHT].tabs, [folder("b"), folder("elsewhere")]);
    assert_eq!(now.panes[RIGHT].active, 0);
    assert_eq!(now.active_pane, RIGHT);
    assert!(now.show_hidden);
}

#[test]
fn a_waiting_tab_goes_to_its_folder_once_it_is_found() {
    let home = folder("home");
    let restored = session(&["a", "home", "home"], 0, &["home"], 0);
    let waiting = [
        Waiting {
            side: LEFT,
            tab: 1,
            path: folder("slow"),
        },
        Waiting {
            side: LEFT,
            tab: 2,
            path: folder("slow"),
        },
        Waiting {
            side: RIGHT,
            tab: 0,
            path: folder("offline"),
        },
    ];
    let (mut state, _) = AppState::restore(
        folder("l"),
        folder("r"),
        restored,
        &waiting,
        Some(home.clone()),
    );
    // Until they are found, the settings keep the folders the tabs stand in for.
    assert_eq!(
        state.session().panes[LEFT].tabs,
        [folder("a"), folder("slow"), folder("slow")]
    );
    assert_eq!(state.session().panes[RIGHT].tabs, [folder("offline")]);

    // The user moves the third tab on meanwhile: it stays where it went.
    let _ = state.select_tab(LEFT, 2);
    let _ = state.go_to(folder("mine"));

    let effects = state.restored_folder(&folder("slow"), true);

    assert_eq!(the_load(&effects).path, folder("slow"));
    assert_eq!(
        locations(&state, LEFT),
        [folder("a"), folder("slow"), folder("mine")]
    );
    // A folder that is not there leaves its tab at home, for the settings too.
    assert_eq!(state.restored_folder(&folder("offline"), false), []);
    assert_eq!(locations(&state, RIGHT), std::slice::from_ref(&home));
    assert_eq!(state.session().panes[RIGHT].tabs, [home]);
    // Each answer counts once.
    assert_eq!(state.restored_folder(&folder("slow"), true), []);
}
