//! Opening the tabs of the last run, and handing them back for the settings.

use super::*;
use crate::settings::{Found, Session, SessionPane, Waiting};

fn session(left: &[&str], left_active: usize, right: &[&str], active_pane: usize) -> Session {
    let pane = |names: &[&str], active| SessionPane {
        tabs: names.iter().map(|name| folder(name)).collect(),
        active,
        pinned: Vec::new(),
    };
    Session {
        panes: [pane(left, left_active), pane(right, 0)],
        active_pane,
        show_hidden: true,
        layout: Layout::default(),
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
    // The second one's listing of home is reloaded meanwhile: it still stands in.
    let _ = state.select_tab(LEFT, 1);
    let _ = state.retry(LEFT);

    let effects = state.restored_folder(&folder("slow"), Found::Folder);

    assert_eq!(the_load(&effects).path, folder("slow"));
    assert_eq!(
        locations(&state, LEFT),
        [folder("a"), folder("slow"), folder("mine")]
    );
    // Each answer counts once.
    assert_eq!(state.restored_folder(&folder("slow"), Found::Folder), []);

    // A folder whose drive is still not there: the tab stays at home and keeps it.
    assert_eq!(
        state.restored_folder(&folder("offline"), Found::Offline),
        []
    );
    assert_eq!(locations(&state, RIGHT), std::slice::from_ref(&home));
    assert_eq!(state.session().panes[RIGHT].tabs, [folder("offline")]);
    // A folder that is gone: the tab stays at home, for the settings too.
    assert_eq!(state.restored_folder(&folder("offline"), Found::Gone), []);
    assert_eq!(
        state.session().panes[RIGHT].tabs,
        std::slice::from_ref(&home)
    );
}

#[test]
fn a_tab_stops_standing_in_once_the_user_moves_it() {
    let home = folder("home");
    let waiting = [Waiting {
        side: RIGHT,
        tab: 0,
        path: folder("usb"),
    }];
    let (mut state, _) = AppState::restore(
        folder("l"),
        folder("r"),
        session(&["a"], 0, &["home"], 1),
        &waiting,
        Some(home.clone()),
    );
    // Choosing home again in that tab makes it a home tab.
    let _ = state.go_to(home.clone());
    assert_eq!(
        state.session().panes[RIGHT].tabs,
        std::slice::from_ref(&home)
    );
    assert_eq!(state.restored_folder(&folder("usb"), Found::Folder), []);
}

#[test]
fn a_start_folder_opens_as_a_new_left_tab() {
    let start = folder("start");
    // A new tab after the left pane's tabs, which keeps the keyboard.
    let opened = session(&["a", "b"], 0, &["c"], 1)
        .with_start(start.clone(), None, &mut Vec::new())
        .0;
    let (state, effects) = AppState::restore(folder("l"), folder("r"), opened, &[], None);
    assert_eq!(
        locations(&state, LEFT),
        [folder("a"), folder("b"), start.clone()]
    );
    assert_eq!(state.panes()[LEFT].active(), 2);
    assert_eq!(state.active(), LEFT);
    assert_eq!(loads(&effects).len(), 4);
    assert_eq!(state.session().panes[LEFT].tabs.len(), 3);

    // A left tab that shows it already shows: no second one.
    let opened = session(&["a", "start"], 0, &["c"], 1)
        .with_start(start.clone(), None, &mut Vec::new())
        .0;
    let (state, _) = AppState::restore(folder("l"), folder("r"), opened, &[], None);
    assert_eq!(locations(&state, LEFT), [folder("a"), start.clone()]);
    assert_eq!(state.panes()[LEFT].active(), 1);
    assert_eq!(state.active(), LEFT);

    // A tab that shows it only standing in for another folder is not it: that tab goes to
    // its own folder once it is found.
    let mut waiting = vec![Waiting {
        side: LEFT,
        tab: 0,
        path: folder("usb"),
    }];
    let (opened, shown) =
        session(&["start"], 0, &[], 0).with_start(start.clone(), None, &mut waiting);
    assert!(shown);
    assert_eq!(opened.panes[LEFT].tabs, [start.clone(), start.clone()]);
    assert_eq!(opened.panes[LEFT].active, 1);
    assert_eq!(waiting.len(), 1);

    // A tab that stands in for the start folder itself (not known in time: a slow drive)
    // shows it at once, as asked, instead of a second tab of it, which would come at every
    // start; it no longer waits.
    let mut waiting = vec![
        Waiting {
            side: LEFT,
            tab: 1,
            path: start.clone(),
        },
        Waiting {
            side: RIGHT,
            tab: 0,
            path: start.clone(),
        },
    ];
    let (opened, shown) =
        session(&["a", "home"], 0, &["home"], 1).with_start(start.clone(), None, &mut waiting);
    assert!(shown);
    assert_eq!(opened.panes[LEFT].tabs, [folder("a"), start.clone()]);
    assert_eq!((opened.panes[LEFT].active, opened.active_pane), (1, LEFT));
    assert_eq!(waiting.iter().map(|w| w.side).collect::<Vec<_>>(), [RIGHT]);

    // A start folder that is gone adds no tab to the saved ones: it would add one at every
    // start, each showing where the gone folder's tab went.
    let (opened, shown) =
        session(&["a"], 0, &["c"], 1).with_start(start.clone(), Some(Found::Gone), &mut Vec::new());
    assert!(!shown);
    assert_eq!(opened.panes[LEFT].tabs, [folder("a")]);
    assert_eq!(opened.active_pane, 1);
    // Unless the pane has no tab: then it is the pane's, which goes where it can.
    let (opened, shown) =
        Session::default().with_start(start.clone(), Some(Found::Gone), &mut Vec::new());
    assert!(shown);
    assert_eq!(opened.panes[LEFT].tabs, [start]);

    // Without saved tabs the left pane has the start folder alone.
    let opened = Session::default()
        .with_start(folder("only"), None, &mut Vec::new())
        .0;
    assert_eq!(opened.panes[LEFT].tabs, [folder("only")]);
}
