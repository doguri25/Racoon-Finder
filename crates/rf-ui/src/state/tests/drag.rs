//! Dragging rows: what a press readies, which places take the items, and the job a drop
//! submits.

use rf_jobs::{ConflictPolicy, JobSpec, TransferItem, TransferMode};

use super::*;
use crate::dialogs::Modal;
use crate::jobs::{JobEffect, Started};
use crate::pick::Validity;
use crate::words::JobWork;

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn in_b(name: &str) -> VPath {
    folder("b").join(name).expect("a plain name")
}

/// The one job `effects` submit, and the pane that asked for it.
fn the_submit(effects: &[Effect]) -> (JobSpec, Option<usize>) {
    let submits: Vec<(JobSpec, Option<usize>)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Job(JobEffect::Submit { spec, origin }) => Some((spec.clone(), *origin)),
            _ => None,
        })
        .collect();
    match submits.as_slice() {
        [one] => one.clone(),
        other => panic!("expected one job, got {other:?} in {effects:?}"),
    }
}

/// A window whose left pane at `a` holds the folders `docs` and `src` and the file `x.txt`,
/// and whose right pane at `b` holds the folder `백업` and the file `y.txt`.
fn two_panes() -> AppState {
    ready(
        vec![dir("docs"), dir("src"), file("x.txt")],
        vec![dir("백업"), file("y.txt")],
    )
}

/// What a drop of the drag readied last on `spot` would do: the folder the items would go
/// into, or `None` when the place does not take them.
fn offer(state: &mut AppState, spot: DropSpot) -> Option<VPath> {
    let id = state.drag_id();
    state.drop_offer(id, spot).map(|offer| offer.folder)
}

const LEFT_LIST: DropSpot = DropSpot::Pane {
    side: LEFT,
    row: None,
};
const RIGHT_LIST: DropSpot = DropSpot::Pane {
    side: RIGHT,
    row: None,
};

#[test]
fn a_press_on_a_selected_row_keeps_the_selection_for_the_drag() {
    let mut state = two_panes();
    state.press_row(LEFT, 0, false, false);
    state.release_row(LEFT, 0);
    state.press_row(LEFT, 2, true, false);
    state.release_row(LEFT, 2);
    assert_eq!(selected(&state, LEFT), [0, 2]);

    // The press on a selected row moves the cursor and leaves the selection for the drag.
    state.activate(RIGHT);
    state.press_row(LEFT, 2, false, false);
    assert_eq!(state.active(), LEFT);
    assert_eq!(selected(&state, LEFT), [0, 2]);
    assert_eq!(cursor(&state, LEFT), Some(2));
    let drag = state.drag().expect("a drag is ready");
    assert_eq!(drag.side, LEFT);
    assert_eq!(drag.request.sources, [in_a("docs"), in_a("x.txt")]);
    assert_eq!(drag.request.from_dir, folder("a"));
    // Released without a drag, it is the click: the row alone is selected.
    state.release_row(LEFT, 2);
    assert_eq!(selected(&state, LEFT), [2]);

    // A press on a row that is not selected is the click at once, and the drag carries it.
    state.press_row(LEFT, 1, false, false);
    assert_eq!(selected(&state, LEFT), [1]);
    assert_eq!(
        state.drag().map(|drag| drag.request.sources.clone()),
        Some(vec![in_a("src")])
    );
    // Its release has nothing left to do.
    state.release_row(LEFT, 1);
    assert_eq!(selected(&state, LEFT), [1]);
}

#[test]
fn ctrl_on_a_selected_row_flips_it_only_when_no_drag_came() {
    let mut state = two_panes();
    press(&mut state, "a", CTRL);
    // Ctrl held for a copy: the press keeps every selected row in the drag.
    state.press_row(LEFT, 1, true, false);
    assert_eq!(selected(&state, LEFT), [0, 1, 2]);
    assert_eq!(state.drag().map(|drag| drag.request.sources.len()), Some(3));
    // Released without a drag, it is the Ctrl+click: the row is no longer selected.
    state.release_row(LEFT, 1);
    assert_eq!(selected(&state, LEFT), [0, 2]);

    // Ctrl on a row that is not selected adds it at once; Shift selects the range at once.
    state.press_row(LEFT, 1, true, false);
    assert_eq!(selected(&state, LEFT), [0, 1, 2]);
    state.press_row(LEFT, 0, false, false);
    state.release_row(LEFT, 0);
    state.press_row(LEFT, 1, false, true);
    assert_eq!(selected(&state, LEFT), [0, 1]);
    state.release_row(LEFT, 1);
    assert_eq!(selected(&state, LEFT), [0, 1]);
}

#[test]
fn a_release_after_the_rows_changed_clicks_the_row_it_pressed() {
    let mut state = two_panes();
    press(&mut state, "a", CTRL);
    state.press_row(LEFT, 2, false, false);
    // A new item comes before `x.txt` meanwhile: the row the press was on moved down.
    let generation = state.panes()[LEFT].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(file("w.txt"))],
    });
    assert_eq!(rows(&state, LEFT), ["docs", "src", "w.txt", "x.txt"]);
    // A release on the row now there is not the press's.
    state.release_row(LEFT, 2);
    assert_eq!(selected(&state, LEFT), [0, 1, 3]);
    // The drag still carries the items of its press.
    assert_eq!(
        state.drag().map(|drag| drag.request.sources.clone()),
        Some(vec![in_a("docs"), in_a("src"), in_a("x.txt")])
    );
}

#[test]
fn a_drag_is_refused_where_it_would_do_nothing_or_go_into_itself() {
    let mut state = two_panes();
    state.press_row(LEFT, 0, false, false);

    // The other pane takes the folder, and so do its folder rows; its file rows stand for
    // the pane's folder.
    assert_eq!(offer(&mut state, RIGHT_LIST), Some(folder("b")));
    let on_right = |row| DropSpot::Pane {
        side: RIGHT,
        row: Some(row),
    };
    assert_eq!(offer(&mut state, on_right(0)), Some(in_b("백업")));
    assert_eq!(offer(&mut state, on_right(1)), Some(folder("b")));
    // So does another folder of its own pane.
    let on_left = |row| DropSpot::Pane {
        side: LEFT,
        row: Some(row),
    };
    assert_eq!(offer(&mut state, on_left(1)), Some(in_a("src")));
    // Not the folder the items are in (DND-09), even through a file row of it, nor the
    // dragged folder itself.
    assert_eq!(offer(&mut state, LEFT_LIST), None);
    assert_eq!(offer(&mut state, on_left(2)), None);
    assert_eq!(offer(&mut state, on_left(0)), None);

    // Not a folder inside the dragged one, nor the folder they are in shown as a row of its
    // parent: the right pane shows `a/docs/deep`, then the parent of `a`.
    let deep = in_a("docs").join("deep").expect("a plain name");
    state.activate(RIGHT);
    let effects = state.go_to(deep.clone());
    let request = the_load(&effects);
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("deeper")],
    });
    assert_eq!(offer(&mut state, RIGHT_LIST), None);
    assert_eq!(offer(&mut state, on_right(0)), None);
    let parent = folder("a").parent().expect("a has a parent");
    let request = the_load(&state.go_to(parent.clone()));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("a"), dir("b")],
    });
    assert_eq!(offer(&mut state, on_right(0)), None);
    assert_eq!(offer(&mut state, on_right(1)), Some(folder("b")));
    assert_eq!(offer(&mut state, RIGHT_LIST), Some(parent));
}

#[test]
fn tabs_and_tree_rows_take_their_folders() {
    let mut state = two_panes();
    // The right pane's second tab shows `a`, where the items are.
    state.add_tab(RIGHT);
    let request = the_load(&state.go_to(folder("a")));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![],
    });
    state.set_places(&[Place {
        label: "문서".to_owned(),
        path: folder("b"),
        kind: PlaceKind::Favorite(rf_platform::KnownFolder::Documents),
        space: None,
    }]);
    state.press_row(LEFT, 2, false, false);

    let tab = |index| DropSpot::Tab { side: RIGHT, index };
    assert_eq!(offer(&mut state, tab(0)), Some(folder("b")));
    assert_eq!(offer(&mut state, tab(1)), None);
    assert_eq!(offer(&mut state, tab(2)), None);
    assert_eq!(offer(&mut state, DropSpot::Tree(0)), Some(folder("b")));
    assert_eq!(offer(&mut state, DropSpot::Tree(9)), None);
}

#[test]
fn a_drop_submits_the_transfer_of_the_items_the_drag_started_with() {
    let mut state = two_panes();
    press(&mut state, "a", CTRL);
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // The selection changes after the drag started (a watch, the keyboard): the drop takes
    // the items of the press.
    press(&mut state, &special(Key::Escape), NONE);
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());

    let spot = DropSpot::Pane {
        side: RIGHT,
        row: Some(0),
    };
    let offered = state.drop_offer(id, spot).expect("the folder takes them");
    assert_eq!((offered.folder, offered.count), (in_b("백업"), 3));
    let (spec, origin) = the_submit(&state.drop_items(id, spot, TransferMode::Move));
    assert_eq!(origin, Some(LEFT));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(transfer.mode, TransferMode::Move);
    assert_eq!(transfer.options.conflict, ConflictPolicy::Ask);
    let target = |name: &str| in_b("백업").join(name).expect("a plain name");
    assert_eq!(
        transfer.items,
        ["docs", "src", "x.txt"].map(|name| TransferItem {
            source: in_a(name),
            target: target(name),
        })
    );
    // One drop each: the drag is over.
    assert_eq!(state.drop_offer(id, spot), None);
    assert_eq!(state.drop_items(id, spot, TransferMode::Move), []);
}

#[test]
fn a_press_where_the_state_has_no_row_readies_no_drag() {
    let mut state = two_panes();
    state.press_row(LEFT, 0, false, false);
    assert_ne!(state.drag_id(), 0);
    // The window drew a row the state no longer has (it went a moment ago): the drag that
    // follows carries nothing, not the items of the press before.
    state.press_row(LEFT, 99, false, false);
    assert_eq!(state.drag_id(), 0);
    assert_eq!(state.drag(), None);
}

#[test]
fn a_drop_goes_only_where_the_window_showed() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let first_row = DropSpot::Pane {
        side: RIGHT,
        row: Some(0),
    };
    assert_eq!(offer(&mut state, first_row), Some(in_b("백업")));

    // A folder made in the right pane comes first under the still pointer (its watch): the
    // first row is another folder than the one the window showed. Nothing moves there.
    let right = state.panes()[RIGHT].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation: right,
        changes: vec![rf_vfs::Change::Upsert(dir("가나"))],
    });
    assert_eq!(
        state.drop_items(id, first_row, TransferMode::Move),
        [Effect::Notice(words::drop_moved().to_owned())]
    );
    // The next hover shows the folder, and a drop then goes there.
    assert_eq!(offer(&mut state, first_row), Some(in_b("가나")));
    let (spec, _) = the_submit(&state.drop_items(id, first_row, TransferMode::Move));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(
        transfer.items[0].target,
        in_b("가나").join("x.txt").expect("a name")
    );
}

#[test]
fn a_drop_after_its_tab_moved_on_or_closed_takes_the_items_it_started_with() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // The tab the drag came from goes elsewhere before the drop.
    let _ = state.go_to(folder("c"));
    let (spec, origin) = the_submit(&state.drop_items(id, RIGHT_LIST, TransferMode::Move));
    assert_eq!(origin, Some(LEFT));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(
        transfer.items,
        [TransferItem {
            source: in_a("x.txt"),
            target: in_b("x.txt"),
        }]
    );

    // A drag from a tab that closes before the drop.
    let mut state = two_panes();
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    let _ = state.add_tab(LEFT);
    let _ = state.close_tab(LEFT, 0);
    let (spec, origin) = the_submit(&state.drop_items(id, RIGHT_LIST, TransferMode::Copy));
    assert_eq!(origin, Some(LEFT));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(
        transfer.items,
        [TransferItem {
            source: in_a("docs"),
            target: in_b("docs"),
        }]
    );
}

#[test]
fn a_copy_goes_where_a_move_would() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // A copy into the folder the item is in would do nothing either (DND-09).
    assert_eq!(state.drop_items(id, LEFT_LIST, TransferMode::Copy), []);
    let (spec, _) = the_submit(&state.drop_items(id, RIGHT_LIST, TransferMode::Copy));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(transfer.mode, TransferMode::Copy);
    assert_eq!(
        transfer.items,
        [TransferItem {
            source: in_a("x.txt"),
            target: in_b("x.txt"),
        }]
    );
}

#[test]
fn only_the_payload_of_the_last_drag_drops_and_only_on_the_main_window() {
    let mut state = two_panes();
    // Before any press, nothing is readied: the payload's 0 takes nothing.
    assert_eq!(state.drag_id(), 0);
    assert_eq!(state.drop_offer(0, RIGHT_LIST), None);
    state.press_row(LEFT, 0, false, false);
    let first = state.drag_id();
    state.press_row(LEFT, 1, false, false);
    let second = state.drag_id();
    assert_ne!(first, second);
    assert_eq!(state.drop_offer(first, RIGHT_LIST), None);
    assert_eq!(state.drop_items(first, RIGHT_LIST, TransferMode::Move), []);
    assert!(state.drop_offer(second, RIGHT_LIST).is_some());

    // While a window shows over the main window, nothing takes a drop.
    state.key(Command::NewFolder, NONE, 1, Instant::now());
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    assert_eq!(state.drop_offer(second, RIGHT_LIST), None);
    assert_eq!(state.drop_items(second, RIGHT_LIST, TransferMode::Move), []);
    state.dialog_cancelled();
    assert!(state.drop_offer(second, RIGHT_LIST).is_some());
}

#[test]
fn a_dragged_move_tells_what_it_moved_and_offers_its_undo() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let (spec, origin) = the_submit(&state.drop_items(id, RIGHT_LIST, TransferMode::Move));
    let record = UndoRecord::Moved {
        moves: vec![rf_jobs::MovedItem {
            from: in_a("x.txt"),
            to: in_b("x.txt"),
        }],
        created_dirs: Vec::new(),
        removed_dirs: Vec::new(),
        overwritten: 0,
        copied_dirs: Vec::new(),
    };
    state.set_history(History {
        top: Some(record.clone()),
        can_redo: false,
    });
    state.track(Started {
        id: JobId(1),
        spec: Some(Arc::new(spec.clone())),
        work: JobWork::Move,
        origin,
        via_history: false,
    });
    let mut report = JobReport::new(JobId(1), &spec);
    report.undo = Some(record);
    let effects = state.job_event(rf_jobs::JobEvent::Finished {
        id: JobId(1),
        report: Arc::new(report),
    });
    // DND-07: the toast says what moved, and its button undoes it.
    let toast = effects.iter().find_map(|effect| match effect {
        Effect::Toast {
            token,
            text,
            action,
        } => Some((*token, text.clone(), action.clone())),
        _ => None,
    });
    let (token, text, action) = toast.expect("a toast");
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 1개를 이동했습니다", "되돌리기")
    );
    assert_eq!(state.toast_clicked(token), [Effect::Job(JobEffect::Undo)]);
}

#[test]
fn a_drop_where_the_rows_changed_to_a_folder_that_refuses_says_so() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // The right pane shows the parent of `a`, which has only `b` yet.
    state.activate(RIGHT);
    let parent = folder("a").parent().expect("a has a parent");
    let request = the_load(&state.go_to(parent));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("b")],
    });
    let first_row = DropSpot::Pane {
        side: RIGHT,
        row: Some(0),
    };
    assert_eq!(offer(&mut state, first_row), Some(folder("b")));
    // `a` comes first under the still pointer: the folder the item is in, which takes nothing.
    state.loaded(LoadEvent::Changes {
        generation: request.generation,
        changes: vec![rf_vfs::Change::Upsert(dir("a"))],
    });
    assert_eq!(
        state.drop_items(id, first_row, TransferMode::Move),
        [Effect::Notice(words::drop_moved().to_owned())]
    );
    // A drop the window never offered says nothing.
    assert_eq!(state.drop_items(id, first_row, TransferMode::Move), []);
}

#[test]
fn a_place_that_refuses_the_items_says_why() {
    let mut state = two_panes();
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    let on_left = |row| DropSpot::Pane {
        side: LEFT,
        row: Some(row),
    };
    // The folder the items are in, and the dragged folder itself.
    assert_eq!(
        state.drop_refused(id, LEFT_LIST),
        Some(Validity::SameFolder)
    );
    assert_eq!(
        state.drop_refused(id, on_left(0)),
        Some(Validity::IntoSource)
    );
    // A place that takes them refuses nothing, and neither does a payload of another drag.
    assert_eq!(state.drop_refused(id, RIGHT_LIST), None);
    assert_eq!(state.drop_refused(id + 1, LEFT_LIST), None);
    assert_eq!(
        words::drop_refused(Validity::SameFolder),
        "여기에 놓을 수 없습니다: 항목이 이미 이 폴더에 있습니다"
    );
    assert_eq!(
        words::drop_refused(Validity::IntoSource),
        "여기에 놓을 수 없습니다: 끌고 있는 폴더 자신이거나 그 안입니다"
    );
}
