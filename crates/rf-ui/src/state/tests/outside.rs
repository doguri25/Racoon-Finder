//! Files dragged from another app onto the window (DND-05 받기): which of its paths are taken,
//! where a drop goes and what it does there, the menu of a drop that asks, this app's own drag
//! back through the system, the undo of a move, and a rest on a folder.

use std::path::PathBuf;

use rf_jobs::{ConflictPolicy, JobSpec, TransferItem, TransferMode};
use rf_platform::dnd::{DragMods, DropEffect, DroppedItems, Effects};

use super::drag::{
    LEFT_LIST, RIGHT_LIST, copy_keys, in_a, in_b, link_keys, run_move, submitted_mode, the_submit,
    toasts, two_panes,
};
use super::*;
use crate::dialogs::Modal;
use crate::settings::DragDefault;

/// An item of the folder `c`, which neither pane shows: where another app drags from.
fn in_c(name: &str) -> VPath {
    folder("c").join(name).expect("a plain name")
}

/// `path` as another app hands it over.
fn path_of(path: &VPath) -> PathBuf {
    path.as_local().expect("a local path").to_path_buf()
}

/// What another app drags: the items at `paths`.
fn items(paths: &[VPath]) -> DroppedItems {
    DroppedItems {
        paths: paths.iter().map(path_of).collect(),
        ours: None,
    }
}

/// A drag that allows a copy and a move, as File Explorer's and Finder's do.
fn both() -> Effects {
    Effects::COPY | Effects::MOVE
}

fn plain() -> DragMods {
    DragMods::default()
}

/// The items of the one transfer `effects` submit.
fn moved_items(effects: &[Effect]) -> Vec<TransferItem> {
    match the_submit(effects).0 {
        JobSpec::Transfer(transfer) => transfer.items,
        spec => panic!("a transfer: {spec:?}"),
    }
}

const BACKUP: DropSpot = DropSpot::Pane {
    side: RIGHT,
    row: Some(0),
};

#[test]
fn a_drop_from_another_app_does_what_the_keys_it_tells_and_the_setting_choose() {
    let mut state = two_panes();
    let z = in_c("z.txt");
    let dragged = items(std::slice::from_ref(&z));
    // A move without a key (the setting's default), which the drag allows: the pointer shows it,
    // and the drop moves the item with the app's own job, which asks about a name that is taken.
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, plain(), both()),
        Some(DropEffect::Move)
    );
    let (done, effects) =
        state.outside_drop(Some(RIGHT_LIST), dragged.clone(), plain(), both(), false);
    assert_eq!(done, Some(DropEffect::Move));
    let JobSpec::Transfer(transfer) = the_submit(&effects).0 else {
        panic!("a transfer: {effects:?}");
    };
    assert_eq!(transfer.mode, TransferMode::Move);
    assert_eq!(transfer.options.conflict, ConflictPolicy::Ask);
    assert_eq!(
        transfer.items,
        [TransferItem {
            source: z.clone(),
            target: in_b("z.txt"),
        }]
    );

    // A drag that allows only a copy (the system's drag on Linux, which does not say) copies.
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, plain(), Effects::COPY),
        Some(DropEffect::Copy)
    );
    let (done, effects) = state.outside_drop(
        Some(RIGHT_LIST),
        dragged.clone(),
        plain(),
        Effects::COPY,
        false,
    );
    assert_eq!(
        (done, submitted_mode(&effects)),
        (Some(DropEffect::Copy), TransferMode::Copy)
    );

    // The keys the system tells choose, never the ones the window heard last (winit forgets
    // them while another app has the focus, as it has during its drag).
    state.modifiers_changed(link_keys());
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, plain(), both()),
        Some(DropEffect::Move)
    );
    state.modifiers_changed(plain());
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, copy_keys(), both()),
        Some(DropEffect::Copy)
    );
    let (done, effects) = state.outside_drop(
        Some(RIGHT_LIST),
        dragged.clone(),
        copy_keys(),
        both(),
        false,
    );
    assert_eq!(
        (done, submitted_mode(&effects)),
        (Some(DropEffect::Copy), TransferMode::Copy)
    );
    // The keys of a link refuse it: no links are made yet.
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, link_keys(), both()),
        None
    );
    assert_eq!(
        state.outside_drop(
            Some(RIGHT_LIST),
            dragged.clone(),
            link_keys(),
            both(),
            false
        ),
        (None, Vec::new())
    );

    // Without a key the setting chooses.
    state.set_drag_default(DragDefault::Copy);
    let (done, effects) =
        state.outside_drop(Some(RIGHT_LIST), dragged.clone(), plain(), both(), false);
    assert_eq!(
        (done, submitted_mode(&effects)),
        (Some(DropEffect::Copy), TransferMode::Copy)
    );
    // A tab and a row of the tree take it into their folders, as they take a drag of rows.
    state.set_drag_default(DragDefault::Move);
    let tab = DropSpot::Tab {
        side: LEFT,
        index: 0,
    };
    let (_, effects) = state.outside_drop(Some(tab), dragged.clone(), plain(), both(), false);
    assert_eq!(moved_items(&effects)[0].target, in_a("z.txt"));
    // Where nothing takes drops, nothing happens.
    assert_eq!(state.outside_over(None, &dragged, plain(), both()), None);
    assert_eq!(
        state.outside_drop(None, dragged, plain(), both(), false),
        (None, Vec::new())
    );
}

#[test]
fn the_paths_another_app_drops_are_checked_as_a_pastes_are() {
    let mut state = two_panes();
    // On Windows a volume shadow copy's path stays a device path; elsewhere it is not even an
    // absolute path. A relative path is no place either.
    let shadow = PathBuf::from(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\a.txt");
    let relative = PathBuf::from("relative.txt");
    let z = in_c("z.txt");
    let mixed = DroppedItems {
        paths: vec![shadow.clone(), path_of(&z), relative.clone()],
        ours: None,
    };
    // The others go, and a notice tells how many were left out.
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &mixed, plain(), both()),
        Some(DropEffect::Move)
    );
    let (done, effects) = state.outside_drop(Some(RIGHT_LIST), mixed, plain(), both(), false);
    assert_eq!(done, Some(DropEffect::Move));
    assert_eq!(
        moved_items(&effects),
        [TransferItem {
            source: z,
            target: in_b("z.txt"),
        }]
    );
    assert!(effects.contains(&Effect::Notice(words::dropped_left_out(2))));

    // Nothing but such paths: refused, and the drop says why.
    let unusable = DroppedItems {
        paths: vec![shadow, relative],
        ours: None,
    };
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &unusable, plain(), both()),
        None
    );
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), unusable, plain(), both(), false),
        (None, vec![Effect::Notice(words::dropped_skipped(2))])
    );
    // No path at all (a drag of something else): nothing, and nothing to say.
    let empty = DroppedItems::default();
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &empty, plain(), both()),
        None
    );
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), empty, plain(), both(), true),
        (None, Vec::new())
    );
    assert_eq!(
        words::dropped_left_out(2),
        "끌어 온 항목 2개는 받을 수 없는 경로라 건너뛰었습니다"
    );
    assert_eq!(
        words::dropped_skipped(2),
        "끌어 온 항목 2개가 모두 받을 수 없는 경로라 받지 않았습니다"
    );
}

#[test]
fn a_drop_while_a_dialog_shows_is_refused() {
    let mut state = two_panes();
    let dragged = items(&[in_c("z.txt")]);
    state.key(Command::NewFolder, NONE, Paging::rows(1), Instant::now());
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    // The pointer shows that nothing takes it, and a drop does nothing, nor shows a menu.
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, plain(), both()),
        None
    );
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), dragged.clone(), plain(), both(), false),
        (None, Vec::new())
    );
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), dragged.clone(), plain(), both(), true),
        (None, Vec::new())
    );
    // Once it has closed, the folder takes it.
    state.dialog_cancelled();
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, plain(), both()),
        Some(DropEffect::Move)
    );
}

#[test]
fn a_drop_into_the_folder_the_items_are_in_does_nothing() {
    let mut state = two_panes();
    // `y.txt` of the right pane's folder, dragged from another app back onto that folder: a
    // move or a copy, with the keys or without, and the right button's menu.
    let here = items(&[in_b("y.txt")]);
    for (mods, allowed) in [
        (plain(), both()),
        (copy_keys(), both()),
        (plain(), Effects::COPY),
    ] {
        assert_eq!(
            state.outside_over(Some(RIGHT_LIST), &here, mods, allowed),
            None,
            "{mods:?}"
        );
        assert_eq!(
            state.outside_drop(Some(RIGHT_LIST), here.clone(), mods, allowed, false),
            (None, Vec::new())
        );
    }
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), here.clone(), plain(), both(), true),
        (None, Vec::new())
    );
    // A folder inside it takes them.
    assert_eq!(
        state.outside_over(Some(BACKUP), &here, plain(), both()),
        Some(DropEffect::Move)
    );

    // A folder dragged onto itself, or into a folder inside it: refused.
    let b = items(&[folder("b")]);
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &b, plain(), both()),
        None
    );
    assert_eq!(state.outside_over(Some(BACKUP), &b, plain(), both()), None);
    assert_eq!(
        state.outside_drop(Some(BACKUP), b, plain(), both(), false),
        (None, Vec::new())
    );
    let backup = items(&[in_b("백업")]);
    assert_eq!(
        state.outside_over(Some(BACKUP), &backup, plain(), both()),
        None
    );

    // Items of two folders: the ones already there stay where they are, the others go.
    let mixed = items(&[in_b("y.txt"), in_c("z.txt")]);
    let (done, effects) = state.outside_drop(Some(RIGHT_LIST), mixed, plain(), both(), false);
    assert_eq!(done, Some(DropEffect::Move));
    assert_eq!(
        moved_items(&effects),
        [TransferItem {
            source: in_c("z.txt"),
            target: in_b("z.txt"),
        }]
    );
}

#[test]
fn a_drop_with_the_right_button_asks_with_the_menu_of_a_drop_that_asks() {
    let mut state = two_panes();
    let z = in_c("z.txt");
    let dragged = items(std::slice::from_ref(&z));
    // File Explorer's drag with the right button: the menu, whatever the keys and the setting;
    // no job yet, and the system is told that nothing was done.
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), dragged.clone(), copy_keys(), both(), true),
        (None, vec![Effect::ShowMenu(MenuShown::AskDrop)])
    );
    let (spec, _) = the_submit(&state.drop_menu_chosen(Some(TransferMode::Move)));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(transfer.mode, TransferMode::Move);
    assert_eq!(transfer.options.conflict, ConflictPolicy::Ask);
    assert_eq!(
        transfer.items,
        [TransferItem {
            source: z,
            target: in_b("z.txt"),
        }]
    );
    // The menu answers once.
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);

    // 매번 묻기 asks too. A drag that allows only a copy copies for 여기로 이동 (DND-05: no move,
    // a copy); one that allows only a move makes nothing of 여기로 복사.
    state.set_drag_default(DragDefault::Ask);
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, plain(), Effects::COPY),
        Some(DropEffect::Copy)
    );
    assert_eq!(
        state.outside_drop(
            Some(RIGHT_LIST),
            dragged.clone(),
            plain(),
            Effects::COPY,
            false
        ),
        (None, vec![Effect::ShowMenu(MenuShown::AskDrop)])
    );
    assert_eq!(
        submitted_mode(&state.drop_menu_chosen(Some(TransferMode::Move))),
        TransferMode::Copy
    );
    assert_eq!(
        state.outside_drop(
            Some(RIGHT_LIST),
            dragged.clone(),
            plain(),
            Effects::MOVE,
            false
        ),
        (None, vec![Effect::ShowMenu(MenuShown::AskDrop)])
    );
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Copy)), []);

    // Its drop does not outlive the menu: closed choosing nothing (Esc), it is forgotten; a
    // window over the main one lets the menu's choice make nothing.
    let ask = |state: &mut AppState| {
        assert_eq!(
            state.outside_drop(Some(RIGHT_LIST), dragged.clone(), plain(), both(), true),
            (None, vec![Effect::ShowMenu(MenuShown::AskDrop)])
        );
    };
    ask(&mut state);
    assert_eq!(state.drop_menu_chosen(None), []);
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
    ask(&mut state);
    state.key(Command::NewFolder, NONE, Paging::rows(1), Instant::now());
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
    state.dialog_cancelled();
    assert_eq!(state.drop_menu_chosen(Some(TransferMode::Move)), []);
}

#[test]
fn this_apps_own_drag_back_through_the_system_drops_as_a_drag_in_the_window() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // The system carries the id of the drag (and paths, which it need not: the drag's own
    // items go).
    let back = DroppedItems {
        paths: Vec::new(),
        ours: Some(id),
    };
    // It hovers as the drag in the window does, with the keys the system tells: the pane the
    // items are in takes nothing.
    assert_eq!(
        state.outside_over(Some(LEFT_LIST), &back, plain(), both()),
        None
    );
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &back, copy_keys(), both()),
        Some(DropEffect::Copy)
    );
    // Its drop is a drop of the drag in the window: the items of its press, from their pane.
    let (done, effects) =
        state.outside_drop(Some(RIGHT_LIST), back.clone(), plain(), both(), false);
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
    // One drop each: the drag is over, and nothing is left to drop.
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), back, plain(), both(), false),
        (None, Vec::new())
    );

    // With the right button, its menu; 여기로 복사 copies the drag's items.
    state.press_row(LEFT, 2, false, false);
    let back = DroppedItems {
        paths: Vec::new(),
        ours: Some(state.drag_id()),
    };
    assert_eq!(
        state.outside_drop(Some(RIGHT_LIST), back, plain(), both(), true),
        (None, vec![Effect::ShowMenu(MenuShown::AskDrop)])
    );
    let (spec, origin) = the_submit(&state.drop_menu_chosen(Some(TransferMode::Copy)));
    assert_eq!(origin, Some(LEFT));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer: {spec:?}");
    };
    assert_eq!(
        (transfer.mode, transfer.items[0].source.clone()),
        (TransferMode::Copy, in_a("x.txt"))
    );

    // The id of a drag that is no longer this app's: its paths go, as another app's would.
    let stale = DroppedItems {
        paths: vec![path_of(&in_c("z.txt"))],
        ours: Some(id),
    };
    let (_, effects) = state.outside_drop(Some(RIGHT_LIST), stale, plain(), both(), false);
    assert_eq!(moved_items(&effects)[0].source, in_c("z.txt"));
}

#[test]
fn a_move_from_another_app_offers_its_undo_for_a_moment() {
    let mut state = two_panes();
    let (_, dropped) = state.outside_drop(
        Some(RIGHT_LIST),
        items(&[in_c("z.txt")]),
        plain(),
        both(),
        false,
    );
    let effects = run_move(&mut state, &dropped, JobId(1), |_| {});
    let shown = toasts(&effects);
    let [(token, text, action, time)] = shown.as_slice() else {
        panic!("one toast: {effects:?}");
    };
    assert_eq!(
        (text.as_str(), action.as_str(), *time),
        ("항목 1개를 옮겼습니다", "되돌리기", Some(MOVE_TOAST_TIME))
    );
    assert_eq!(state.toast_clicked(*token), [Effect::Job(JobEffect::Undo)]);
}

#[test]
fn a_rest_of_a_drag_from_another_app_opens_only_the_folder_it_showed() {
    let mut state = two_panes();
    let dragged = items(&[in_c("z.txt")]);
    // Before a hover there, nothing opens.
    assert_eq!(state.drag_rests(BACKUP), []);
    assert_eq!(
        state.outside_over(Some(BACKUP), &dragged, plain(), both()),
        Some(DropEffect::Move)
    );
    // A folder comes before `백업` under the still pointer: its row is another folder's when
    // the time is up, which no hover showed, and nothing opens.
    let generation = state.panes()[RIGHT].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(dir("old"))],
    });
    assert_eq!(rows(&state, RIGHT)[0], "old");
    assert_eq!(state.drag_rests(BACKUP), []);
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), folder("b"));
    // A drop there now says that the place changed, and makes nothing.
    assert_eq!(
        state.outside_drop(Some(BACKUP), dragged.clone(), plain(), both(), false),
        (None, vec![Effect::Notice(words::drop_moved().to_owned())])
    );
    // Once a hover shows `old` there, its time opens it; the drag goes on, the left pane
    // keeps the keys.
    assert_eq!(
        state.outside_over(Some(BACKUP), &dragged, plain(), both()),
        Some(DropEffect::Move)
    );
    let request = the_load(&state.drag_rests(BACKUP));
    assert_eq!(request.path, in_b("old"));
    assert_eq!(state.active(), LEFT);
    // A place the keys refuse counts for nothing, and neither does a drag gone from the window.
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("deep")],
    });
    assert_eq!(
        state.outside_over(Some(BACKUP), &dragged, link_keys(), both()),
        None
    );
    assert_eq!(state.drag_rests(BACKUP), []);
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), in_b("old"));
    assert_eq!(
        state.outside_over(Some(BACKUP), &dragged, plain(), both()),
        Some(DropEffect::Move)
    );
    state.outside_leave();
    assert_eq!(state.drag_rests(BACKUP), []);
    assert_eq!(*state.panes()[RIGHT].active_tab().location(), in_b("old"));
    // A rest of a drag in the window still opens what its own hover showed.
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    assert!(state.drop_offer(id, BACKUP).is_some());
    let request = the_load(&state.drag_rests(BACKUP));
    assert_eq!(
        request.path,
        in_b("old").join("deep").expect("a plain name")
    );
}
