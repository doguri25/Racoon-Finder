//! The destination picker: opening it, choosing in it, its tree, its 새 폴더 and the job it
//! submits.

use rf_jobs::{ConflictPolicy, CreateKind, JobSpec, Outcome, TransferMode};

use std::path::MAIN_SEPARATOR;

use super::*;
use crate::dialogs::{Modal, NamePurpose};
use crate::jobs::{JobEffect, Started};
use crate::pick::{Picker, TargetFacts, Validity};
use crate::settings::PickStart;
use crate::words::JobWork;

const MB: u64 = 1024 * 1024;

fn sized(name: &str, size: u64) -> Entry {
    let mut entry = file(name);
    entry.size = size;
    entry
}

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn in_b(name: &str) -> VPath {
    folder("b").join(name).expect("a plain name")
}

/// A window whose left pane at `a` holds the folder `docs` and two files, all selected.
fn with_items() -> AppState {
    let mut state = ready(
        vec![dir("docs"), sized("x.txt", 2 * MB), sized("y.txt", MB)],
        vec![dir("백업")],
    );
    press(&mut state, "a", CTRL);
    state
}

fn picker(state: &AppState) -> &Picker {
    match state.modal() {
        Some(Modal::Pick(picker)) => picker,
        other => panic!("the picker shows: {other:?}"),
    }
}

/// The one look at a target among `effects`: its generation and its path.
fn the_look(effects: &[Effect]) -> (u64, VPath) {
    let looks: Vec<(u64, VPath)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::InspectTarget {
                generation,
                path,
                from_dir,
            } => {
                assert_eq!(*from_dir, folder("a"));
                Some((*generation, path.clone()))
            }
            _ => None,
        })
        .collect();
    match looks.as_slice() {
        [one] => one.clone(),
        other => panic!("expected one look, got {other:?} in {effects:?}"),
    }
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

fn folder_facts() -> TargetFacts {
    TargetFacts {
        folder: Ok(()),
        writable: true,
        available: Some(512 * MB),
        same_volume: false,
        other_volume: false,
    }
}

/// The places of the picker's tree: `b` as a favorite.
fn places() -> Vec<Place> {
    vec![Place {
        label: "문서".to_owned(),
        path: folder("b"),
        kind: PlaceKind::Favorite(rf_platform::KnownFolder::Documents),
        space: None,
    }]
}

/// The listings of folders `effects` ask for.
fn tree_loads(effects: &[Effect]) -> Vec<LoadRequest> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LoadChildren(request) => Some(request.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn f5_and_f6_open_the_picker_at_the_other_pane() {
    let mut state = with_items();
    let effects = press(&mut state, &special(Key::F8), NONE).expect("the key is used");
    let shown = picker(&state);
    assert_eq!(shown.request.mode, TransferMode::Move);
    assert_eq!(
        shown.request.sources,
        [in_a("docs"), in_a("x.txt"), in_a("y.txt")]
    );
    assert_eq!(
        (shown.request.file_bytes, shown.request.folder_count),
        (3 * MB, 1)
    );
    assert_eq!(shown.request.from_dir, folder("a"));
    assert_eq!(shown.origin, LEFT);
    // The other pane's folder is chosen (PICK-05), and a worker looks at it.
    assert_eq!(shown.target, Some(folder("b")));
    assert_eq!(the_look(&effects).1, folder("b"));
    assert_eq!(shown.groups[0].title, "반대편 패널");
    // The items' own folder, a tab of the left pane, cannot take a move.
    assert_eq!(shown.groups[1].items[0].path, folder("a"));
    assert_eq!(shown.groups[1].items[0].validity, Validity::SameFolder);

    // While it shows, the panes get no key.
    let before = cursor(&state, LEFT);
    assert_eq!(
        press(&mut state, &special(Key::DownArrow), NONE),
        Some(vec![])
    );
    assert_eq!(press(&mut state, &special(Key::F7), NONE), Some(vec![]));
    assert_eq!(cursor(&state, LEFT), before);
    assert_eq!(picker(&state).request.mode, TransferMode::Move);
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);

    // F7 copies; the toolbar's buttons and the row menu send the same commands.
    state.key(Command::CopyTo, NONE, Paging::rows(1), Instant::now());
    assert_eq!(picker(&state).request.mode, TransferMode::Copy);
    state.dialog_cancelled();

    // In an empty folder, nothing opens; nor does it for a row the user unselected, which the
    // screen shows unselected.
    let mut empty = ready(vec![], vec![]);
    assert_eq!(press(&mut empty, &special(Key::F7), NONE), Some(vec![]));
    assert_eq!(empty.modal(), None);
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    state.click_row(LEFT, 1, false, false);
    state.click_row(LEFT, 1, true, false);
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    assert_eq!(press(&mut state, &special(Key::F7), NONE), Some(vec![]));
    assert_eq!(state.modal(), None);
}

#[test]
fn confirming_submits_the_transfer_and_remembers_the_target() {
    let mut state = with_items();
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    let (look, b) = the_look(&effects);
    assert_eq!(state.target_inspected(look, &b, folder_facts()), []);
    state.pick_policy(ConflictPolicy::Skip);
    let (spec, origin) = the_submit(&state.pick_confirmed());
    assert_eq!(origin, Some(LEFT));
    let JobSpec::Transfer(transfer) = spec else {
        panic!("a transfer");
    };
    assert_eq!(transfer.mode, TransferMode::Copy);
    assert_eq!(transfer.options.conflict, ConflictPolicy::Skip);
    let targets: Vec<&VPath> = transfer.items.iter().map(|item| &item.target).collect();
    assert_eq!(targets, [&in_b("docs"), &in_b("x.txt"), &in_b("y.txt")]);
    assert_eq!(state.modal(), None);
    assert_eq!(state.recent_targets(), std::slice::from_ref(&b));

    // A target typed in comes first among the recent ones, and the next picker offers it.
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    let (look, _) = the_look(&effects);
    let c = folder("c");
    let effects = state.pick_path_entered(&format!("  {c}  "));
    let (typed, path) = the_look(&effects);
    assert_eq!(path, c);
    // The earlier look is late now.
    assert_eq!(state.target_inspected(look, &b, folder_facts()), []);
    state.target_inspected(typed, &c, folder_facts());
    assert_eq!(picker(&state).target, Some(c.clone()));
    // Enter on the path of the target confirms, as Enter does elsewhere.
    let (spec, _) = the_submit(&state.pick_path_entered(&c.to_string()));
    assert!(matches!(spec, JobSpec::Transfer(_)));
    assert_eq!(state.recent_targets(), [c.clone(), b.clone()]);
    press(&mut state, &special(Key::F7), NONE);
    let recent = picker(&state)
        .groups
        .iter()
        .find(|group| group.title == "최근 위치")
        .expect("the recent targets");
    // `b` shows once, as the other pane's folder.
    assert_eq!(recent.items.len(), 1);
    assert_eq!(recent.items[0].path, c);

    // Kept from an earlier run: each once, ten at most.
    let many: Vec<VPath> = (0..12).map(|i| folder(&format!("r{i}"))).collect();
    let mut with_repeat = vec![many[0].clone()];
    with_repeat.extend(many.iter().cloned());
    state.set_recent_targets(with_repeat);
    assert_eq!(state.recent_targets(), &many[..10]);
}

#[test]
fn a_target_that_cannot_take_the_items_is_never_confirmed() {
    let mut state = with_items();
    press(&mut state, &special(Key::F8), NONE);
    // The items' own folder, typed: a move there would do nothing.
    let effects = state.pick_path_entered(&folder("a").to_string());
    let (look, a) = the_look(&effects);
    state.target_inspected(look, &a, folder_facts());
    assert_eq!(picker(&state).validity(), Some(Validity::SameFolder));
    assert_eq!(state.pick_confirmed(), []);
    assert_eq!(state.pick_enter(), []);
    assert!(matches!(state.modal(), Some(Modal::Pick(_))));
    // A copy there is fine.
    state.pick_mode(TransferMode::Copy);
    assert!(picker(&state).can_confirm());
    state.pick_mode(TransferMode::Move);

    // Inside a folder that moves: not even a click on the quick target takes it.
    let effects = state.pick_path_entered(&in_a("docs").join("sub").expect("a name").to_string());
    let (look, inside) = the_look(&effects);
    state.target_inspected(look, &inside, folder_facts());
    assert_eq!(picker(&state).validity(), Some(Validity::IntoSource));
    assert_eq!(state.pick_confirmed(), []);
    let blocked = picker(&state)
        .targets()
        .position(|item| !item.validity.can_choose())
        .expect("the items' folder among the tabs");
    assert_eq!(state.pick_clicked(blocked), []);

    // A path that is no folder stays out, and the field says why; text that is no path too.
    let effects = state.pick_path_entered(&in_a("x.txt").to_string());
    let (look, file_path) = the_look(&effects);
    let not_a_folder = TargetFacts {
        folder: Err(ErrorKind::NotADirectory),
        ..folder_facts()
    };
    assert_eq!(state.target_inspected(look, &file_path, not_a_folder), []);
    assert_eq!(picker(&state).target, Some(inside));
    assert_eq!(picker(&state).path_error, "폴더가 아닙니다");
    assert_eq!(state.pick_path_entered("relative/path"), []);
    assert_eq!(picker(&state).path_error, "쓸 수 없는 이름이나 경로입니다");

    // Too little room only warns: the button still works.
    press(&mut state, &special(Key::Escape), NONE);
    state.dialog_cancelled();
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    let (look, b) = the_look(&effects);
    let full = TargetFacts {
        available: Some(MB),
        ..folder_facts()
    };
    state.target_inspected(look, &b, full);
    assert_eq!(
        picker(&state).validity(),
        Some(Validity::NotEnoughSpace {
            need: 3 * MB,
            available: MB
        })
    );
    assert!(matches!(
        the_submit(&state.pick_confirmed()).0,
        JobSpec::Transfer(_)
    ));
}

#[test]
fn an_overwrite_is_confirmed_by_the_button_alone() {
    let mut state = with_items();
    press(&mut state, &special(Key::F7), NONE);
    state.pick_policy(ConflictPolicy::Overwrite);
    assert_eq!(state.pick_enter(), []);
    assert_eq!(state.pick_path_entered(&folder("b").to_string()), []);
    assert!(matches!(state.modal(), Some(Modal::Pick(_))));
    let (JobSpec::Transfer(transfer), _) = the_submit(&state.pick_confirmed()) else {
        panic!("a transfer");
    };
    assert_eq!(transfer.options.conflict, ConflictPolicy::Overwrite);
}

#[test]
fn a_conditional_overwrite_is_confirmed_by_the_button_alone_too() {
    for policy in [
        ConflictPolicy::OverwriteIfNewer,
        ConflictPolicy::OverwriteIfSizeDiffers,
    ] {
        let mut state = with_items();
        press(&mut state, &special(Key::F7), NONE);
        state.pick_policy(policy);
        // It may replace files: Enter, in the filter or on the target's own path, does not
        // confirm it.
        assert_eq!(state.pick_enter(), []);
        assert_eq!(state.pick_path_entered(&folder("b").to_string()), []);
        assert!(matches!(state.modal(), Some(Modal::Pick(_))));
        // The button does, and the job runs under the policy as it was chosen.
        let (JobSpec::Transfer(transfer), _) = the_submit(&state.pick_confirmed()) else {
            panic!("a transfer");
        };
        assert_eq!(transfer.options.conflict, policy);
    }
}

#[test]
fn a_copy_follows_links_only_when_its_box_is_on() {
    use rf_jobs::SymlinkMode;

    let symlinks = |effects: &[Effect]| match the_submit(effects).0 {
        JobSpec::Transfer(transfer) => (transfer.mode, transfer.options.symlinks),
        other => panic!("a transfer: {other:?}"),
    };
    // A copy: the box is off at first, and a link is copied as a link.
    let mut state = with_items();
    press(&mut state, &special(Key::F7), NONE);
    assert!(picker(&state).offers_follow_links() && !picker(&state).follow_links);
    assert_eq!(
        symlinks(&state.pick_confirmed()),
        (TransferMode::Copy, SymlinkMode::CopyLink)
    );
    // Flipped on, the copy follows links.
    press(&mut state, &special(Key::F7), NONE);
    state.pick_follow_links_toggled();
    assert!(picker(&state).follow_links);
    assert_eq!(
        symlinks(&state.pick_confirmed()),
        (TransferMode::Copy, SymlinkMode::Follow)
    );
    // Each picker starts with it off again.
    press(&mut state, &special(Key::F7), NONE);
    assert!(!picker(&state).follow_links);
    state.pick_follow_links_toggled();
    state.pick_follow_links_toggled();
    assert!(!picker(&state).follow_links);
    state.dialog_cancelled();

    // A move has no box: a press that came anyway flips nothing, and a box that was on while
    // the picker copied does not make the move follow links.
    press(&mut state, &special(Key::F8), NONE);
    assert!(!picker(&state).offers_follow_links());
    state.pick_follow_links_toggled();
    assert!(!picker(&state).follow_links);
    state.pick_mode(TransferMode::Copy);
    state.pick_follow_links_toggled();
    assert!(picker(&state).follow_links);
    state.pick_mode(TransferMode::Move);
    assert_eq!(
        symlinks(&state.pick_confirmed()),
        (TransferMode::Move, SymlinkMode::CopyLink)
    );
}

#[test]
fn arrows_move_among_the_quick_targets() {
    let mut state = with_items();
    state.add_tab(RIGHT);
    let c = folder("c");
    let request = the_load(&state.go_to(c.clone()));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![],
    });
    state.activate(LEFT);
    press(&mut state, &special(Key::F8), NONE);
    // c (the other pane's tab that shows), then the tabs: a (the items' folder, passed over)
    // and b.
    assert_eq!(picker(&state).target, Some(c.clone()));
    let effects = state.pick_step(1);
    assert_eq!(the_look(&effects).1, folder("b"));
    assert_eq!(picker(&state).target, Some(folder("b")));
    assert_eq!(state.pick_step(1), []);
    assert_eq!(the_look(&state.pick_step(-1)).1, c);
}

#[test]
fn the_filter_finds_listed_folders_and_enter_takes_the_first() {
    let mut state = with_items();
    state.set_places(&places());
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    // The picker's tree opens on the way to the target, a place here: it is selected as it is.
    assert_eq!(state.pick_tree().selected_row(), Some(0));
    assert!(tree_loads(&effects).is_empty(), "{effects:?}");
    let request = state
        .pick_tree_toggled(0)
        .into_iter()
        .find_map(|effect| match effect {
            Effect::LoadChildren(request) => Some(request),
            _ => None,
        })
        .expect("the place is listed");
    assert!(state.generations().contains(&request.generation));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("백업"), dir("사진"), dir("백업 2025")],
    });
    assert_eq!(state.pick_tree().rows().len(), 4);

    assert_eq!(state.pick_filter("백업"), []);
    let found = state.pick_found().expect("a filter");
    assert_eq!(found.len(), 2);
    // Enter takes the first folder found, and confirms the next time.
    let effects = state.pick_enter();
    assert_eq!(the_look(&effects).1, in_b("백업"));
    let (JobSpec::Transfer(transfer), _) = the_submit(&state.pick_enter()) else {
        panic!("a transfer");
    };
    assert_eq!(
        transfer.items[0].target,
        in_b("백업").join("docs").expect("a name")
    );

    // A click on a row the filter found takes that folder; a filter that finds nothing does
    // nothing on Enter.
    press(&mut state, &special(Key::F7), NONE);
    let request = tree_loads(&state.pick_tree_toggled(0)).remove(0);
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("백업"), dir("사진")],
    });
    state.pick_filter("사");
    assert_eq!(the_look(&state.pick_tree_clicked(0)).1, in_b("사진"));
    assert_eq!(state.pick_tree_toggled(0), []);
    state.pick_filter("없는 이름");
    assert_eq!(state.pick_found(), Some(vec![]));
    assert_eq!(state.pick_enter(), []);
    // Without the filter the tree shows as a tree again, at the target's row.
    assert_eq!(state.pick_filter(""), [Effect::ScrollPickTree(2)]);
    assert_eq!(state.pick_found(), None);
}

#[test]
fn the_pickers_new_folder_becomes_its_target() {
    let mut state = with_items();
    press(&mut state, &special(Key::F8), NONE);
    assert_eq!(state.pick_new_folder(), []);
    // The name window shows over the picker, which stays on screen.
    let Some(Modal::Name(dialog)) = state.modal() else {
        panic!("a name window: {:?}", state.modal());
    };
    assert_eq!(
        dialog.purpose,
        NamePurpose::PickFolder {
            parent: folder("b")
        }
    );
    assert!(state.shown_picker().is_some());
    // The picker takes nothing while the name window has the keys.
    assert_eq!(state.pick_step(1), []);
    assert_eq!(state.pick_confirmed(), []);

    let (spec, origin) = the_submit(&state.name_confirmed("자료"));
    let made = in_b("자료");
    assert_eq!(
        spec,
        JobSpec::Create {
            parent: folder("b"),
            name: "자료".into(),
            kind: CreateKind::Dir,
        }
    );
    // The folder belongs to no pane: neither selects it.
    assert_eq!(origin, None);
    assert!(matches!(state.modal(), Some(Modal::Pick(_))));

    state.track(Started {
        id: JobId(4),
        spec: Some(Arc::new(spec.clone())),
        work: JobWork::Create,
        origin: None,
        via_history: false,
    });
    let mut report = JobReport::new(JobId(4), &spec);
    report.outputs.push(made.clone());
    let effects = state.job_event(rf_jobs::JobEvent::Finished {
        id: JobId(4),
        report: Arc::new(report),
    });
    assert_eq!(the_look(&effects).1, made);
    assert_eq!(picker(&state).target, Some(made.clone()));
    assert_eq!(picker(&state).making, None);

    // A create that failed leaves the target as it was; Esc in the name window keeps the
    // picker.
    state.pick_new_folder();
    state.dialog_cancelled();
    assert!(matches!(state.modal(), Some(Modal::Pick(_))));
    state.pick_new_folder();
    let (spec, _) = the_submit(&state.name_confirmed("둘째"));
    let mut report = JobReport::new(JobId(5), &spec);
    report.outcome = Outcome::Aborted;
    state.job_event(rf_jobs::JobEvent::Finished {
        id: JobId(5),
        report: Arc::new(report),
    });
    assert_eq!(picker(&state).target, Some(made));
}

#[test]
fn closing_the_picker_stops_its_tree_listings() {
    let mut state = with_items();
    state.set_places(&places());
    press(&mut state, &special(Key::F7), NONE);
    let request = tree_loads(&state.pick_tree_toggled(0)).remove(0);
    assert!(state.generations().contains(&request.generation));
    state.dialog_cancelled();
    assert_eq!(state.modal(), None);
    assert!(!state.generations().contains(&request.generation));
    // Its listing, if it comes anyway, goes nowhere.
    assert_eq!(
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![dir("늦게")],
        }),
        []
    );
    assert_eq!(state.pick_tree().rows().len(), 0);
    // So do late results of its worker.
    assert_eq!(state.target_inspected(1, &folder("b"), folder_facts()), []);
}

#[test]
fn the_button_waits_while_the_path_field_says_otherwise() {
    let mut state = with_items();
    let effects = press(&mut state, &special(Key::F8), NONE).expect("used");
    let (look, b) = the_look(&effects);
    state.target_inspected(look, &b, folder_facts());
    state.pick_policy(ConflictPolicy::Overwrite);
    let c = folder("c");

    // A path typed and not entered: neither the button nor Enter elsewhere confirms the
    // target the field no longer shows.
    state.pick_path_edited(&c.to_string());
    assert_eq!(state.pick_confirmed(), []);
    // Nor is a new folder made in a folder the field does not show.
    assert_eq!(state.pick_new_folder(), []);
    state.pick_policy(ConflictPolicy::Ask);
    assert_eq!(state.pick_enter(), []);
    assert!(matches!(state.modal(), Some(Modal::Pick(_))));
    // Entered: not while the worker looks at it.
    let (typed, path) = the_look(&state.pick_path_entered(&c.to_string()));
    assert_eq!(path, c);
    assert_eq!(state.pick_confirmed(), []);
    // Refused: the field keeps the path, and the button stays off.
    let gone = TargetFacts {
        folder: Err(ErrorKind::NotFound),
        ..folder_facts()
    };
    state.target_inspected(typed, &c, gone);
    assert_eq!(picker(&state).path_error, "찾을 수 없습니다");
    assert_eq!(picker(&state).target, Some(b.clone()));
    assert_eq!(state.pick_confirmed(), []);
    // Typed again and found: the field shows the target, and the button goes there.
    state.pick_path_edited(&c.to_string());
    assert_eq!(picker(&state).path_error, "");
    let (typed, _) = the_look(&state.pick_path_entered(&c.to_string()));
    state.target_inspected(typed, &c, folder_facts());
    let (JobSpec::Transfer(transfer), _) = the_submit(&state.pick_confirmed()) else {
        panic!("a transfer");
    };
    assert_eq!(transfer.items[0].target, c.join("docs").expect("a name"));

    // A target chosen elsewhere replaces what was typed.
    press(&mut state, &special(Key::F8), NONE);
    state.pick_path_edited(&folder("elsewhere").to_string());
    assert_eq!(state.pick_confirmed(), []);
    state.pick_step(1);
    assert!(!picker(&state).path_edited);
    assert!(matches!(
        the_submit(&state.pick_confirmed()).0,
        JobSpec::Transfer(_)
    ));
}

/// A window with the items of `with_items` whose places are `a` itself: the picker's tree
/// can list the folder the items are in.
fn with_items_and_their_folder() -> (AppState, LoadRequest) {
    let mut state = with_items();
    state.set_recent_targets(vec![folder("c")]);
    state.set_places(&[Place {
        label: "작업".to_owned(),
        path: folder("a"),
        kind: PlaceKind::Favorite(rf_platform::KnownFolder::Documents),
        space: None,
    }]);
    press(&mut state, &special(Key::F8), NONE);
    let request = tree_loads(&state.pick_tree_toggled(0)).remove(0);
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("docs"), dir("docs 보관"), dir("사진")],
    });
    (state, request)
}

#[test]
fn the_filter_takes_a_folder_that_can_take_the_items_and_leaves_a_later_choice() {
    let (mut state, _) = with_items_and_their_folder();
    // `docs` moves itself: the first folder found that can take the items is `docs 보관`.
    state.pick_filter("docs");
    assert_eq!(state.pick_found().map(|found| found.len()), Some(2));
    assert_eq!(the_look(&state.pick_enter()).1, in_a("docs 보관"));
    assert_eq!(
        the_submit(&state.pick_enter()).1,
        Some(LEFT),
        "the second Enter confirms"
    );

    // A target chosen after the filter changed stays: Enter confirms it.
    let (mut state, _) = with_items_and_their_folder();
    state.pick_filter("docs");
    // From the other pane's `b`, ↓ goes to the next quick target that can take the items.
    let (look, next) = the_look(&state.pick_step(1));
    assert_ne!(next, folder("b"));
    state.target_inspected(look, &next, folder_facts());
    let (JobSpec::Transfer(transfer), _) = the_submit(&state.pick_enter()) else {
        panic!("a transfer");
    };
    assert_eq!(transfer.items[0].target, next.join("docs").expect("a name"));
}

#[test]
fn the_tree_shows_but_does_not_take_a_folder_that_cannot_take_the_items() {
    let (mut state, _) = with_items_and_their_folder();
    let rows = state.pick_tree().rows();
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, ["작업", "docs", "docs 보관", "사진"]);
    let target = picker(&state).target.clone();
    // The items' own folder (a move there does nothing) and an item: no click takes them.
    assert_eq!(state.pick_tree_clicked(0), []);
    assert_eq!(state.pick_tree_clicked(1), []);
    assert_eq!(picker(&state).target, target);
    assert_eq!(the_look(&state.pick_tree_clicked(2)).1, in_a("docs 보관"));
    // Its chevron still opens it.
    assert_eq!(tree_loads(&state.pick_tree_toggled(1)).len(), 1);
}

#[test]
fn the_picker_starts_at_the_other_pane_or_where_the_setting_says() {
    // The default (PICK-05): the other pane's folder, whatever was chosen last.
    let mut state = with_items();
    state.set_recent_targets(vec![folder("c")]);
    assert_eq!(state.pick_start(), PickStart::Opposite);
    press(&mut state, &special(Key::F8), NONE);
    assert_eq!(picker(&state).target, Some(folder("b")));
    state.dialog_cancelled();
    // 마지막으로 쓴 곳: the last target, among the quick targets still.
    state.set_pick_start(PickStart::Last);
    let effects = press(&mut state, &special(Key::F8), NONE).expect("used");
    assert_eq!(picker(&state).target, Some(folder("c")));
    assert_eq!(the_look(&effects).1, folder("c"));
    assert!(
        picker(&state)
            .targets()
            .any(|item| item.path == folder("c"))
    );
    assert!(
        picker(&state)
            .targets()
            .any(|item| item.path == folder("b"))
    );
    state.dialog_cancelled();
    // A last target that cannot take the items (a move into their own folder) leaves the
    // other pane's chosen; so does having none.
    state.set_recent_targets(vec![folder("a")]);
    press(&mut state, &special(Key::F8), NONE);
    assert_eq!(picker(&state).target, Some(folder("b")));
    state.dialog_cancelled();
    state.set_recent_targets(Vec::new());
    press(&mut state, &special(Key::F7), NONE);
    assert_eq!(picker(&state).target, Some(folder("b")));
}

#[test]
fn the_window_s_new_folder_key_makes_a_folder_in_the_picker() {
    let mut state = with_items();
    press(&mut state, &special(Key::F8), NONE);
    let command = keys::command_for("N", CTRL_SHIFT, Os::Windows).expect("a key");
    assert_eq!(command, Command::NewFolder);
    assert_eq!(state.pick_key(&command), Some(vec![]));
    let Some(Modal::Name(dialog)) = state.modal() else {
        panic!("a name window: {:?}", state.modal());
    };
    assert_eq!(
        dialog.purpose,
        NamePurpose::PickFolder {
            parent: folder("b")
        }
    );
    // Over the picker, the picker takes no key; another key is none of the picker's.
    assert_eq!(state.pick_key(&command), None);
    state.dialog_cancelled();
    assert_eq!(state.pick_key(&Command::Trash), None);
    state.dialog_cancelled();
    assert_eq!(state.pick_key(&command), None);
}

#[test]
fn tab_completes_the_picker_s_path_as_the_path_field_does() {
    let mut state = with_items();
    press(&mut state, &special(Key::F8), NONE);
    let a = folder("a").to_string();
    let typed = format!("{a}{MAIN_SEPARATOR}d");
    let effects = state.pick_path_edited(&typed);
    let request = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::ListNames(request) => Some(request.clone()),
            _ => None,
        })
        .expect("the names are listed");
    assert_eq!(
        (request.folder.clone(), request.prefix.as_str()),
        (folder("a"), "d")
    );
    // A Tab before the names come waits for them, and the field is filled once they do.
    assert_eq!(state.pick_complete_path(&typed), None);
    assert!(state.pick_completion_waits(&typed));
    let effects = state.loaded(LoadEvent::Names {
        generation: request.generation,
        names: vec![
            crate::loader::FolderName::folder("docs"),
            crate::loader::FolderName::folder("dump"),
        ],
        more: 0,
        links_left: false,
    });
    let completed = format!("{a}{MAIN_SEPARATOR}docs{MAIN_SEPARATOR}");
    assert!(
        effects.contains(&Effect::FillPickPath(completed.clone())),
        "{effects:?}"
    );
    // The field holds a path typed and not entered: nothing is confirmed until Enter.
    assert!(picker(&state).path_edited);
    assert_eq!(state.pick_confirmed(), []);
    // After a separator there is nothing to complete: Tab moves on.
    assert_eq!(state.pick_complete_path(&completed), None);
    assert!(!state.pick_completion_waits(&completed));
    // Typed again, the names are there: Tab completes at once with the first of them.
    let typed = format!("{a}{MAIN_SEPARATOR}du");
    let request = the_names(&mut state, &typed);
    state.loaded(LoadEvent::Names {
        generation: request,
        names: vec![crate::loader::FolderName::folder("dump")],
        more: 0,
        links_left: false,
    });
    let (text, _) = state.pick_complete_path(&typed).expect("completed");
    assert_eq!(text, format!("{a}{MAIN_SEPARATOR}dump{MAIN_SEPARATOR}"));
    // A start no folder has: nothing, and Tab moves on.
    let typed = format!("{a}{MAIN_SEPARATOR}zz");
    let request = the_names(&mut state, &typed);
    state.loaded(LoadEvent::Names {
        generation: request,
        names: Vec::new(),
        more: 0,
        links_left: false,
    });
    assert_eq!(state.pick_complete_path(&typed), None);
    assert!(!state.pick_completion_waits(&typed));
    // The pane's path field has no part of it.
    assert_eq!(state.completion(LEFT), None);
    // Once the picker closes, its completion goes with it.
    state.dialog_cancelled();
    state.drop_stale_address();
    assert_eq!(state.pick_complete_path(&typed), None);
}

/// The generation of the listing of names the picker's path field asks for as it holds
/// `typed`.
fn the_names(state: &mut AppState, typed: &str) -> u64 {
    state
        .pick_path_edited(typed)
        .iter()
        .find_map(|effect| match effect {
            Effect::ListNames(request) => Some(request.generation),
            _ => None,
        })
        .expect("the names are listed")
}

#[test]
fn a_last_place_that_is_gone_gives_way_to_the_other_pane() {
    let mut state = with_items();
    state.set_recent_targets(vec![folder("c")]);
    state.set_pick_start(PickStart::Last);
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    let (look, path) = the_look(&effects);
    assert_eq!(path, folder("c"));
    assert!(picker(&state).from_last);
    // The worker finds it gone: the picker goes to the other pane's folder, and looks there.
    let gone = TargetFacts {
        folder: Err(ErrorKind::NotFound),
        ..folder_facts()
    };
    let effects = state.target_inspected(look, &folder("c"), gone.clone());
    let (look, path) = the_look(&effects);
    assert_eq!(path, folder("b"));
    assert_eq!(picker(&state).target, Some(folder("b")));
    assert!(!picker(&state).from_last);
    // Enter confirms there once it is found to be a folder.
    state.target_inspected(look, &folder("b"), folder_facts());
    let (JobSpec::Transfer(transfer), _) = the_submit(&state.pick_enter()) else {
        panic!("a transfer");
    };
    assert_eq!(transfer.items[0].target, in_b("docs"));

    // A folder the user chose or typed is not replaced, gone or not: only the setting's.
    state.set_recent_targets(vec![folder("c")]);
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    let (look, _) = the_look(&effects);
    let (chosen, _) = the_look(&state.pick_step(-1));
    assert!(!picker(&state).from_last);
    // The late look at the last place says nothing now.
    assert_eq!(state.target_inspected(look, &folder("c"), gone.clone()), []);
    let target = picker(&state).target.clone().expect("a target");
    assert_eq!(state.target_inspected(chosen, &target, gone.clone()), []);
    assert_eq!(picker(&state).target, Some(target));
    state.dialog_cancelled();
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    let (look, _) = the_look(&effects);
    let typed = folder("typed");
    let (typing, _) = the_look(&state.pick_path_entered(&typed.to_string()));
    state.target_inspected(typing, &typed, folder_facts());
    assert_eq!(picker(&state).target, Some(typed.clone()));
    assert!(!picker(&state).from_last);
    assert_eq!(state.target_inspected(look, &folder("c"), gone), []);
    assert_eq!(picker(&state).target, Some(typed));
}

/// The picker of `with_items` opened by F7 with the last place `c` chosen by the setting, and
/// the generation of the look at `c`, still out.
fn starting_at_a_gone_place() -> (AppState, u64) {
    let mut state = with_items();
    state.set_recent_targets(vec![folder("c")]);
    state.set_pick_start(PickStart::Last);
    let effects = press(&mut state, &special(Key::F7), NONE).expect("used");
    let (look, path) = the_look(&effects);
    assert_eq!(path, folder("c"));
    assert!(picker(&state).from_last);
    (state, look)
}

fn gone() -> TargetFacts {
    TargetFacts {
        folder: Err(ErrorKind::NotFound),
        ..folder_facts()
    }
}

/// Whether `effects` ask a worker to look at any path.
fn looks(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::InspectTarget { .. }))
}

#[test]
fn a_path_typed_before_the_gone_place_is_found_gone_stays() {
    let (mut state, look) = starting_at_a_gone_place();
    // The user types in the path field while the look at c is still out (a share offline).
    let typed = folder("백업").to_string();
    state.pick_path_edited(&typed);
    assert!(!picker(&state).from_last);
    // The look comes back: nothing is chosen in the place of what the field holds.
    let effects = state.target_inspected(look, &folder("c"), gone());
    assert!(!looks(&effects), "{effects:?}");
    assert!(picker(&state).path_edited);
    assert_eq!(picker(&state).target, Some(folder("c")));
    // Nothing is confirmed until the path typed is entered.
    assert_eq!(state.pick_confirmed(), []);
    assert_eq!(state.pick_enter(), []);
}

#[test]
fn a_path_entered_before_the_gone_place_is_found_gone_has_its_own_look() {
    let (mut state, look) = starting_at_a_gone_place();
    let entered = folder("백업");
    let (typing, path) = the_look(&state.pick_path_entered(&entered.to_string()));
    assert_eq!(path, entered);
    assert!(!picker(&state).from_last);
    // The look at c comes back first: the entered path's look is still the one waited for.
    let effects = state.target_inspected(look, &folder("c"), gone());
    assert!(!looks(&effects), "{effects:?}");
    assert_eq!(picker(&state).typed, Some(entered.clone()));
    // Then its own: it becomes the target, and Enter copies there.
    state.target_inspected(typing, &entered, folder_facts());
    assert_eq!(picker(&state).target, Some(entered.clone()));
    let (JobSpec::Transfer(transfer), _) = the_submit(&state.pick_enter()) else {
        panic!("a transfer");
    };
    assert_eq!(
        transfer.items[0].target,
        entered.join("docs").expect("a name")
    );
}

#[test]
fn a_filter_typed_before_the_gone_place_is_found_gone_keeps_enter_to_itself() {
    let (mut state, look) = starting_at_a_gone_place();
    state.pick_filter("백");
    assert!(!picker(&state).from_last);
    let effects = state.target_inspected(look, &folder("c"), gone());
    assert!(!looks(&effects), "{effects:?}");
    assert!(picker(&state).filter_fresh);
    assert_eq!(picker(&state).target, Some(folder("c")));
    // A single Enter takes what the filter finds (nothing here), and copies nowhere: not into
    // the other pane.
    let effects = state.pick_enter();
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Job(_))),
        "{effects:?}"
    );
    assert!(matches!(state.modal(), Some(Modal::Pick(_))));
}
