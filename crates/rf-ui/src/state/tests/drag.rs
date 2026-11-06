//! Dragging rows: what a press readies, which places take the items, and the job a drop
//! submits.

use rf_jobs::{ConflictPolicy, JobSpec, TransferItem, TransferMode};
use rf_platform::dnd::{DragMods, DropEffect};

use super::*;
use crate::dialogs::Modal;
use crate::jobs::{JobEffect, Started};
use crate::pick::Validity;
use crate::settings::DragDefault;
use crate::state::drop_rule::DropChoice;
use crate::words::JobWork;

pub(super) fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

pub(super) fn in_b(name: &str) -> VPath {
    folder("b").join(name).expect("a plain name")
}

/// The one job `effects` submit, and the pane that asked for it.
pub(super) fn the_submit(effects: &[Effect]) -> (JobSpec, Option<usize>) {
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

/// The menu of 놓을 때 묻기, offering 여기로 이동 (`moves`), 여기로 복사 (`copies`) and 여기에
/// 링크 만들기 (`links`) or not.
pub(super) fn ask_menu(moves: bool, copies: bool, links: bool) -> Effect {
    Effect::ShowMenu(MenuShown::AskDrop {
        moves,
        copies,
        links,
    })
}

/// A window whose left pane at `a` holds the folders `docs` and `src` and the file `x.txt`,
/// and whose right pane at `b` holds the folder `백업` and the file `y.txt`.
pub(super) fn two_panes() -> AppState {
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

pub(super) const LEFT_LIST: DropSpot = DropSpot::Pane {
    side: LEFT,
    row: None,
};
pub(super) const RIGHT_LIST: DropSpot = DropSpot::Pane {
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
    assert_eq!(offered.folder, in_b("백업"));
    let (spec, origin) = the_submit(&state.drop_items(id, spot));
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
    assert_eq!(state.drop_items(id, spot), []);
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
        state.drop_items(id, first_row),
        [Effect::Notice(words::drop_moved().to_owned())]
    );
    // The next hover shows the folder, and a drop then goes there.
    assert_eq!(offer(&mut state, first_row), Some(in_b("가나")));
    let (spec, _) = the_submit(&state.drop_items(id, first_row));
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
    let (spec, origin) = the_submit(&state.drop_items(id, RIGHT_LIST));
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

    // A drag from a tab that closes before the drop, set to copy.
    let mut state = two_panes();
    state.set_drag_default(DragDefault::Copy);
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    let _ = state.add_tab(LEFT);
    let _ = state.close_tab(LEFT, 0);
    let (spec, origin) = the_submit(&state.drop_items(id, RIGHT_LIST));
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
    state.set_drag_default(DragDefault::Copy);
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // A copy into the folder the item is in would do nothing either (DND-09).
    assert_eq!(state.drop_items(id, LEFT_LIST), []);
    let (spec, _) = the_submit(&state.drop_items(id, RIGHT_LIST));
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
    assert_eq!(state.drop_items(first, RIGHT_LIST), []);
    assert!(state.drop_offer(second, RIGHT_LIST).is_some());

    // While a window shows over the main window, nothing takes a drop.
    state.key(Command::NewFolder, NONE, Paging::rows(1), Instant::now());
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    assert_eq!(state.drop_offer(second, RIGHT_LIST), None);
    assert_eq!(state.drop_items(second, RIGHT_LIST), []);
    state.dialog_cancelled();
    assert!(state.drop_offer(second, RIGHT_LIST).is_some());
}

#[test]
fn a_drop_that_asks_is_forgotten_by_the_next_drag_and_while_a_window_shows() {
    let mut state = two_panes();
    state.set_drag_default(DragDefault::Ask);
    let ask = |state: &mut AppState| {
        state.press_row(LEFT, 2, false, false);
        let id = state.drag_id();
        assert_eq!(
            state.drop_items(id, RIGHT_LIST),
            [ask_menu(true, true, true)]
        );
    };
    // Its menu closed choosing nothing (Esc, a click beside it, which the window tells as
    // 취소), and the next choice has no drop to make.
    ask(&mut state);
    assert_eq!(state.drop_menu_chosen(None), []);
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
    // A press that readies another drag forgets it too.
    ask(&mut state);
    state.press_row(LEFT, 1, false, false);
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
    // While a window shows over the main window, its choice makes nothing, and the drop is
    // forgotten.
    ask(&mut state);
    state.key(Command::NewFolder, NONE, Paging::rows(1), Instant::now());
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
    state.dialog_cancelled();
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
}

/// The record of a move of `x.txt` from `a` into `b`.
fn moved_x() -> UndoRecord {
    UndoRecord::Moved {
        moves: vec![rf_jobs::MovedItem {
            from: in_a("x.txt"),
            to: in_b("x.txt"),
        }],
        created_dirs: Vec::new(),
        removed_dirs: Vec::new(),
        overwritten: 0,
        copied_dirs: Vec::new(),
    }
}

/// The engine starts the job `effects` submit as job `id`, which ends as `finish` leaves its
/// report (it moved `x.txt` from `a` into `b`, and its record is the next one to undo): the
/// effects of its end.
pub(super) fn run_move(
    state: &mut AppState,
    effects: &[Effect],
    id: JobId,
    finish: impl FnOnce(&mut JobReport),
) -> Vec<Effect> {
    let (spec, origin) = the_submit(effects);
    state.set_history(History {
        top: Some(moved_x()),
        can_redo: false,
        ..History::default()
    });
    state.track(Started {
        id,
        spec: Some(Arc::new(spec.clone())),
        work: JobWork::Move,
        origin,
        via_history: false,
    });
    let mut report = JobReport::new(id, &spec);
    report.undo = Some(moved_x());
    finish(&mut report);
    state.job_event(rf_jobs::JobEvent::Finished {
        id,
        report: Arc::new(report),
    })
}

/// The toasts among `effects`: their tokens, texts, buttons and times.
pub(super) fn toasts(effects: &[Effect]) -> Vec<(u64, String, String, Option<Duration>)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Toast {
                token,
                text,
                action,
                time,
            } => Some((*token, text.clone(), action.clone(), *time)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_dragged_move_tells_what_it_moved_and_offers_its_undo() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let dropped = state.drop_items(id, RIGHT_LIST);
    let effects = run_move(&mut state, &dropped, JobId(1), |_| {});
    // DND-07: for 5 seconds the toast says what the drag moved, and its button undoes it with
    // the record the history keeps (which checks for later changes as every undo does).
    let shown = toasts(&effects);
    let [(token, text, action, time)] = shown.as_slice() else {
        panic!("one toast: {effects:?}");
    };
    assert_eq!(
        (text.as_str(), action.as_str(), *time),
        ("항목 1개를 옮겼습니다", "되돌리기", Some(MOVE_TOAST_TIME))
    );
    assert_eq!(MOVE_TOAST_TIME, Duration::from_secs(5));
    assert_eq!(state.toast_clicked(*token), [Effect::Job(JobEffect::Undo)]);

    // A move that no drag made tells it as before, for the window's usual time.
    state.activate(LEFT);
    press(&mut state, &special(Key::Escape), NONE);
    let _ = state.click_row(LEFT, 2, false, false);
    let picked = state.transfer(
        LEFT,
        TransferMode::Move,
        &folder("b"),
        rf_jobs::TransferOptions::default(),
    );
    let effects = run_move(&mut state, &picked, JobId(2), |_| {});
    let shown = toasts(&effects);
    let [(_, text, action, time)] = shown.as_slice() else {
        panic!("one toast: {effects:?}");
    };
    assert_eq!(
        (text.as_str(), action.as_str(), *time),
        ("항목 1개를 이동했습니다", "되돌리기", None)
    );
}

#[test]
fn a_dragged_move_offers_no_undo_when_the_setting_is_off() {
    let mut state = two_panes();
    assert!(state.move_undo_toast());
    state.set_move_undo_toast(false);
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let dropped = state.drop_items(id, RIGHT_LIST);
    let effects = run_move(&mut state, &dropped, JobId(1), |_| {});
    assert_eq!(toasts(&effects), [], "{effects:?}");
    // Ctrl+Z still undoes it.
    assert!(state.can_undo());
}

#[test]
fn a_dragged_move_with_a_failure_offers_no_undo_and_tells_the_failure() {
    let mut state = two_panes();
    state.press_row(LEFT, 0, true, false);
    state.press_row(LEFT, 2, true, false);
    let id = state.drag_id();
    let dropped = state.drop_items(id, RIGHT_LIST);
    let effects = run_move(&mut state, &dropped, JobId(1), |report| {
        report.outcome = rf_jobs::Outcome::CompletedWithErrors;
        report.failures.push(rf_jobs::ItemFailure {
            source: Some(in_a("docs")),
            target: Some(in_b("docs")),
            stage: rf_jobs::Stage::Write,
            kind: ErrorKind::PermissionDenied,
            message: String::new(),
            reason: None,
        });
    });
    // The report of the job's end tells the failure (M1-06), and nothing offers the undo.
    let shown = toasts(&effects);
    let [(_, text, action, time)] = shown.as_slice() else {
        panic!("one toast: {effects:?}");
    };
    assert_eq!(
        (text.as_str(), action.as_str(), *time),
        ("항목 1개를 처리하지 못했습니다", "자세히", None)
    );
}

#[test]
fn a_drag_resting_on_a_folder_row_opens_it_and_drops_by_its_rows() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let backup = DropSpot::Pane {
        side: RIGHT,
        row: Some(0),
    };
    assert_eq!(offer(&mut state, backup), Some(in_b("백업")));
    // DND-08: the right pane opens the folder; the drag goes on, and the left pane keeps the
    // keys.
    let request = the_load(&state.drag_rests(backup));
    assert_eq!(request.path, in_b("백업"));
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), in_b("백업"));
    assert_eq!(state.drag_id(), id);
    assert_eq!(state.active(), LEFT);
    // A drop goes by the rows of the folder opened: its folder `old`, or the folder itself
    // below them.
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("old"), file("z.txt")],
    });
    let old = in_b("백업").join("old").expect("a plain name");
    assert_eq!(offer(&mut state, backup), Some(old));
    assert_eq!(offer(&mut state, RIGHT_LIST), Some(in_b("백업")));
    let (spec, _) = the_submit(&state.drop_items(id, RIGHT_LIST));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(
        transfer.items,
        [TransferItem {
            source: in_a("x.txt"),
            target: in_b("백업").join("x.txt").expect("a plain name"),
        }]
    );
}

#[test]
fn a_drag_resting_on_a_tab_shows_it_and_on_a_closed_tree_folder_opens_it() {
    let mut state = two_panes();
    // The tree shows the place 임시 (which holds both panes' folders) open on a, b and c.
    let mut effects = state.set_places(&[nav::drive("임시", &nav::temp())]);
    effects.extend(state.follow_active());
    let request = nav::the_tree_load(&effects);
    let _ = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("a"), dir("b"), dir("c")],
    });
    // The right pane's second tab shows `c`; its first one shows.
    let _ = state.add_tab(RIGHT);
    let request = the_load(&state.go_to(folder("c")));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![],
    });
    let _ = state.select_tab(RIGHT, 0);
    state.press_row(LEFT, 2, false, false);

    // The timer counts only after a hover there, which shows where a drop goes.
    let rest = |state: &mut AppState, spot: DropSpot| {
        let _ = offer(state, spot);
        state.drag_rests(spot)
    };
    let second = DropSpot::Tab {
        side: RIGHT,
        index: 1,
    };
    assert_eq!(rest(&mut state, second), []);
    assert_eq!(state.panes()[RIGHT].active(), 1);
    assert_eq!(state.active(), LEFT);
    // The tab that shows already stays.
    assert_eq!(rest(&mut state, second), []);
    assert_eq!(state.panes()[RIGHT].active(), 1);

    // The tree's closed `b` opens, and its folders are listed.
    let expanded = |state: &AppState, row: usize| state.tree().rows()[row].expanded;
    assert_eq!(state.tree().rows()[2].label, "b");
    assert!(!expanded(&state, 2));
    let request = nav::the_tree_load(&rest(&mut state, DropSpot::Tree(2)));
    assert_eq!(request.path, folder("b"));
    assert!(expanded(&state, 2));
    // An open folder stays open.
    assert!(expanded(&state, 0));
    assert_eq!(rest(&mut state, DropSpot::Tree(0)), []);
    assert!(expanded(&state, 0));
    // `a`, where the items are, takes nothing, and does not open.
    assert_eq!(state.tree().rows()[1].label, "a");
    assert_eq!(rest(&mut state, DropSpot::Tree(1)), []);
    assert!(!expanded(&state, 1));
}

#[test]
fn a_rest_opens_only_the_folder_its_time_was_counted_for() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let first_row = DropSpot::Pane {
        side: RIGHT,
        row: Some(0),
    };
    // The timer starts for `백업`, which the hover shows.
    assert_eq!(offer(&mut state, first_row), Some(in_b("백업")));
    // A folder comes before it under the still pointer (a watch, a job that ended): its row is
    // another folder's when the time is up, which no hover showed, and nothing opens.
    let generation = state.panes()[RIGHT].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(dir("old"))],
    });
    assert_eq!(rows(&state, RIGHT)[0], "old");
    assert_eq!(state.drag_rests(first_row), []);
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), folder("b"));
    // Without a hover nothing opens either; once one shows `old` there, its own time opens it.
    let mut fresh = two_panes();
    fresh.press_row(LEFT, 2, false, false);
    assert_eq!(fresh.drag_rests(first_row), []);
    assert_eq!(offer(&mut state, first_row), Some(in_b("old")));
    let request = the_load(&state.drag_rests(first_row));
    assert_eq!(request.path, in_b("old"));
}

#[test]
fn a_drag_resting_where_its_items_cannot_go_or_on_no_folder_opens_nothing() {
    let mut state = two_panes();
    let on_right = |row| DropSpot::Pane {
        side: RIGHT,
        row: Some(row),
    };
    // Each rest comes after a hover there, as the timer counts only then.
    let rest = |state: &mut AppState, spot: DropSpot| {
        let _ = offer(state, spot);
        state.drag_rests(spot)
    };
    // No drag yet.
    assert_eq!(rest(&mut state, on_right(0)), []);
    state.press_row(LEFT, 0, false, false);
    // The folder dragged, a row that is no folder, and the list below the rows.
    let docs = DropSpot::Pane {
        side: LEFT,
        row: Some(0),
    };
    assert_eq!(rest(&mut state, docs), []);
    assert_eq!(rest(&mut state, on_right(1)), []);
    assert_eq!(rest(&mut state, RIGHT_LIST), []);
    assert_eq!(*state.panes()[LEFT].active_tab().location(), folder("a"));
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), folder("b"));
    // A tab of the folder the items are in.
    let _ = state.add_tab(RIGHT);
    let request = the_load(&state.go_to(folder("a")));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("docs")],
    });
    let _ = state.select_tab(RIGHT, 0);
    state.press_row(LEFT, 0, false, false);
    let tab_of_a = DropSpot::Tab {
        side: RIGHT,
        index: 1,
    };
    assert_eq!(rest(&mut state, tab_of_a), []);
    assert_eq!(state.panes()[RIGHT].active(), 0);
    // A folder inside the one dragged: the right pane shows `a/docs`, which holds `inner`.
    state.activate(RIGHT);
    let request = the_load(&state.go_to(in_a("docs")));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("inner")],
    });
    state.press_row(LEFT, 0, false, false);
    assert_eq!(rest(&mut state, on_right(0)), []);
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), in_a("docs"));

    // While a window shows over the main window, nothing opens.
    state.activate(RIGHT);
    let request = the_load(&state.go_to(folder("b")));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("백업")],
    });
    state.press_row(LEFT, 2, false, false);
    assert_eq!(offer(&mut state, on_right(0)), Some(in_b("백업")));
    state.key(Command::NewFolder, NONE, Paging::rows(1), Instant::now());
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    assert_eq!(state.drag_rests(on_right(0)), []);
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), folder("b"));
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
        state.drop_items(id, first_row),
        [Effect::Notice(words::drop_moved().to_owned())]
    );
    // A drop the window never offered says nothing.
    assert_eq!(state.drop_items(id, first_row), []);
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

/// The keys that ask a drop for a copy on this system: Ctrl, or ⌥ on macOS.
pub(super) fn copy_keys() -> DragMods {
    if Os::CURRENT == Os::MacOs {
        DragMods {
            alt: true,
            ..DragMods::default()
        }
    } else {
        DragMods {
            ctrl: true,
            ..DragMods::default()
        }
    }
}

/// The keys that ask a drop for a link on this system: Ctrl+Shift, or ⌥⌘ on macOS.
pub(super) fn link_keys() -> DragMods {
    if Os::CURRENT == Os::MacOs {
        DragMods {
            alt: true,
            logo: true,
            ..DragMods::default()
        }
    } else {
        DragMods {
            ctrl: true,
            shift: true,
            ..DragMods::default()
        }
    }
}

/// The mode of the transfer `effects` submit.
pub(super) fn submitted_mode(effects: &[Effect]) -> TransferMode {
    match the_submit(effects).0 {
        JobSpec::Transfer(transfer) => transfer.mode,
        spec => panic!("a transfer: {spec:?}"),
    }
}

#[test]
fn the_keys_held_choose_what_a_drop_does() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let choice = |state: &mut AppState| state.drop_offer(id, RIGHT_LIST).map(|offer| offer.choice);
    assert_eq!(choice(&mut state), Some(DropChoice::Do(DropEffect::Move)));
    // The window tells the keys as they change; each hover asks with the keys held then.
    state.modifiers_changed(copy_keys());
    assert_eq!(choice(&mut state), Some(DropChoice::Do(DropEffect::Copy)));
    state.modifiers_changed(link_keys());
    assert_eq!(choice(&mut state), Some(DropChoice::Do(DropEffect::Link)));
    state.modifiers_changed(copy_keys());
    assert_eq!(
        submitted_mode(&state.drop_items(id, RIGHT_LIST)),
        TransferMode::Copy
    );

    // Let go, the setting chooses again.
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    state.modifiers_changed(DragMods::default());
    assert_eq!(
        submitted_mode(&state.drop_items(id, RIGHT_LIST)),
        TransferMode::Move
    );
}

#[test]
fn the_setting_chooses_a_drop_without_a_key() {
    let mut state = two_panes();
    assert_eq!(state.drag_default(), DragDefault::Move);
    state.set_drag_default(DragDefault::Copy);
    assert_eq!(state.drag_default(), DragDefault::Copy);
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    assert_eq!(
        submitted_mode(&state.drop_items(id, RIGHT_LIST)),
        TransferMode::Copy
    );

    // The system's way: a move within a drive of the sidebar, a copy to another. The drives
    // come from the places a worker found; none known yet counts as another drive.
    state.set_drag_default(DragDefault::System);
    let drive = |path: VPath| Place {
        label: "디스크".to_owned(),
        path,
        kind: PlaceKind::Drive,
        space: None,
    };
    let os_way = |state: &mut AppState| {
        state.press_row(LEFT, 2, false, false);
        let id = state.drag_id();
        state.drop_offer(id, RIGHT_LIST).map(|offer| offer.choice)
    };
    assert_eq!(os_way(&mut state), Some(DropChoice::Do(DropEffect::Copy)));
    let both = VPath::local(std::env::temp_dir()).expect("a local path");
    let _ = state.set_places(&[drive(both)]);
    assert_eq!(os_way(&mut state), Some(DropChoice::Do(DropEffect::Move)));
    // `b` on a drive of its own, `a` on the one that holds both.
    let both = VPath::local(std::env::temp_dir()).expect("a local path");
    let _ = state.set_places(&[drive(both), drive(folder("b"))]);
    assert_eq!(os_way(&mut state), Some(DropChoice::Do(DropEffect::Copy)));
}

#[test]
fn a_drop_that_asks_shows_its_menu_and_the_menu_submits_the_job() {
    let mut state = two_panes();
    state.set_drag_default(DragDefault::Ask);
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let offer = state
        .drop_offer(id, RIGHT_LIST)
        .expect("the folder takes it");
    assert_eq!(offer.choice, DropChoice::Ask);
    // No job yet: the menu shows where the drop was.
    assert_eq!(
        state.drop_items(id, RIGHT_LIST),
        [ask_menu(true, true, true)]
    );
    // One drop each: the drag is over.
    assert_eq!(state.drop_offer(id, RIGHT_LIST), None);
    // 여기로 이동 moves the items the drag carried into the folder of the drop.
    let (spec, origin) = the_submit(&state.drop_menu_chosen(Some(TransferMode::Move)));
    assert_eq!(origin, Some(LEFT));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(transfer.mode, TransferMode::Move);
    assert_eq!(transfer.options.conflict, ConflictPolicy::Ask);
    assert_eq!(
        transfer.items,
        [TransferItem {
            source: in_a("x.txt"),
            target: in_b("x.txt"),
        }]
    );
    // The menu answers once.
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);

    // 여기로 복사 copies; 취소 (and Esc, or a click beside the menu, which choose nothing)
    // does nothing.
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let _ = state.drop_items(id, RIGHT_LIST);
    assert_eq!(
        submitted_mode(&state.drop_menu_chosen(Some(TransferMode::Copy))),
        TransferMode::Copy
    );
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let _ = state.drop_items(id, RIGHT_LIST);
    assert_eq!(state.drop_menu_chosen(None), []);
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
    // A key held at the drop chooses at once, without the menu.
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    state.modifiers_changed(copy_keys());
    assert_eq!(
        submitted_mode(&state.drop_items(id, RIGHT_LIST)),
        TransferMode::Copy
    );
}
