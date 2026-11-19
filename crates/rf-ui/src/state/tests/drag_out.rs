//! Dragging rows out of the window to other apps (DND-05 보내기): which drag goes out, with
//! what, once, and that its end, whatever the app that took it did, makes the app do nothing.

use std::path::PathBuf;

use rf_jobs::{JobSpec, TransferItem, TransferMode};
use rf_platform::dnd::{DropEffect, DroppedItems, Effects};

use super::drag::{LEFT_LIST, RIGHT_LIST, in_a, in_b, the_submit, two_panes};
use super::*;

/// `path` as the system's drag carries it.
fn path_of(path: &VPath) -> PathBuf {
    path.as_local().expect("a local path").to_path_buf()
}

/// Whether `effects` hand anything to the job engine: a job, an undo, a redo, an answer.
fn any_job(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Job(_)))
}

#[test]
fn only_a_drag_of_rows_leaves_the_window_and_only_once() {
    let mut state = two_panes();
    // No press has readied a drag.
    assert_eq!(state.drag_leaves_window(), None);

    // The drag a press readied: the local paths of its items and its id.
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    assert_ne!(id, 0);
    assert_eq!(
        state.drag_leaves_window(),
        Some(DragOut {
            paths: vec![path_of(&in_a("x.txt"))],
            ours: id,
        })
    );
    // The same drag never goes out twice.
    assert_eq!(state.drag_leaves_window(), None);
    // Its id stays the window's: the system's drag that comes back over the window is it.
    assert_eq!(state.drag_id(), id);

    // A new press readies a new drag, which goes out with all the items it carries, in the
    // order of the rows.
    press(&mut state, "a", CTRL);
    state.press_row(LEFT, 0, false, false);
    let out = state.drag_leaves_window().expect("the new drag goes out");
    assert_ne!(out.ours, id);
    assert_eq!(out.ours, state.drag_id());
    assert_eq!(
        out.paths,
        [in_a("docs"), in_a("src"), in_a("x.txt")]
            .iter()
            .map(path_of)
            .collect::<Vec<_>>()
    );

    // A drag dropped in the window is over: nothing is left to go out.
    state.press_row(RIGHT, 1, false, false);
    let id = state.drag_id();
    let (spec, _) = the_submit(&state.drop_items(id, LEFT_LIST));
    assert!(matches!(spec, JobSpec::Transfer(_)), "{spec:?}");
    assert_eq!(state.drag_leaves_window(), None);

    // While a dialog shows over the window, the drag stays the window's.
    state.press_row(LEFT, 2, false, false);
    state.key(Command::NewFolder, NONE, Paging::rows(1), Instant::now());
    assert!(state.modal().is_some());
    assert_eq!(state.drag_leaves_window(), None);
    state.dialog_cancelled();
    assert!(state.drag_leaves_window().is_some());
}

#[test]
fn a_press_whose_drag_left_the_window_clicks_nothing_at_its_release() {
    let mut state = two_panes();
    press(&mut state, "a", CTRL);
    // A press on a selected row keeps the selection for its drag, and leaves its click for
    // the release.
    state.press_row(LEFT, 2, false, false);
    assert!(state.drag_leaves_window().is_some());
    // A drag came of the press: no click is left for a release.
    assert_eq!(state.release_row(LEFT, 2), Vec::new());
    assert_eq!(selected(&state, LEFT), [0, 1, 2]);
}

#[test]
fn a_drag_out_that_ends_as_a_move_deletes_nothing() {
    // Review Focus: whatever the app that took the items did, the app submits no job: no
    // delete of the originals, no move, no copy. A folder's watch shows what that app did.
    for result in [
        Some(DropEffect::Move),
        Some(DropEffect::Copy),
        Some(DropEffect::Link),
        None,
    ] {
        let mut state = two_panes();
        state.press_row(LEFT, 2, false, false);
        let id = state.drag_id();
        assert!(state.drag_leaves_window().is_some(), "{result:?}");
        let effects = state.drag_out_ended(id, result);
        assert!(!any_job(&effects), "{result:?}: {effects:?}");
        assert_eq!(effects, Vec::new(), "{result:?}");
        // The drag is over: it drops nowhere any more, not even in the window, and a second
        // end changes nothing either.
        assert_eq!(state.drag(), None, "{result:?}");
        assert_eq!(state.drop_items(id, RIGHT_LIST), Vec::new(), "{result:?}");
        assert_eq!(state.drag_out_ended(id, result), Vec::new(), "{result:?}");
    }
}

#[test]
fn the_end_of_a_drag_that_never_left_changes_nothing() {
    let mut state = two_panes();
    // Nothing went out: no drag to end.
    assert_eq!(state.drag_out_ended(1, Some(DropEffect::Move)), Vec::new());
    // A drag readied since, which has not gone out, stays ready.
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    assert_eq!(state.drag_out_ended(id, None), Vec::new());
    assert_eq!(state.drag_id(), id);
    assert!(state.drag().is_some());
}

#[test]
fn the_end_of_an_earlier_drag_out_leaves_the_drag_after_it() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let first = state
        .drag_leaves_window()
        .expect("the first drag goes out")
        .ours;
    // Before its end comes back, a new drag is readied and goes out too.
    state.press_row(LEFT, 1, false, false);
    let second = state
        .drag_leaves_window()
        .expect("the second drag goes out")
        .ours;
    assert_ne!(first, second);
    // The end of the first is stale: the second stays the window's, and drops in it should it
    // come back.
    assert_eq!(state.drag_out_ended(first, None), Vec::new());
    assert_eq!(state.drag_id(), second);
    assert!(state.drag().is_some());
    // Its own end is over it.
    assert_eq!(state.drag_out_ended(second, None), Vec::new());
    assert_eq!(state.drag(), None);
    assert_eq!(state.drop_items(second, RIGHT_LIST), Vec::new());
}

#[test]
fn a_drag_out_that_comes_back_moves_once_as_a_drag_in_the_window() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let out = state.drag_leaves_window().expect("the drag goes out");
    // The system's drag comes back over the other pane, carrying the drag's tag.
    let back = DroppedItems {
        paths: out.paths.clone(),
        ours: Some(out.ours),
        ..DroppedItems::default()
    };
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &back, DragMods::default(), Effects::ALL),
        Some(DropEffect::Move)
    );
    // Dropped there, it is the drag of rows: the window's own move, the default without a key.
    let (done, effects) = state.outside_drop(
        Some(RIGHT_LIST),
        back.clone(),
        DragMods::default(),
        Effects::ALL,
        false,
    );
    assert_eq!(done, Some(DropEffect::Move));
    let (spec, origin) = the_submit(&effects);
    assert_eq!(origin, Some(LEFT));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(
        (transfer.mode, transfer.items),
        (
            TransferMode::Move,
            vec![TransferItem {
                source: in_a("x.txt"),
                target: in_b("x.txt"),
            }]
        )
    );
    // Then the system's drag ends: nothing more happens, once or twice.
    assert_eq!(state.drag_out_ended(out.ours, None), Vec::new());
    assert_eq!(
        state.drag_out_ended(out.ours, Some(DropEffect::Move)),
        Vec::new()
    );
    assert_eq!(state.drop_items(out.ours, RIGHT_LIST), Vec::new());
}
