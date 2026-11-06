//! The sidebar's favorites the user keeps (NAV-05, DND-01) and the folders visited last: the
//! first edit that makes the known folders the user's own, adding, removing, renaming and
//! moving, the drops between favorites that add a folder and move nothing, and the group
//! "최근 폴더".

use std::path::PathBuf;

use rf_platform::KnownFolder;
use rf_platform::dnd::{DragMods, DropEffect, DroppedItems, Effects};

use super::drag::{link_keys, the_submit, two_panes};
use super::nav::{drive, temp};
use super::*;
use crate::dialogs::{Modal, NamePurpose};
use crate::keys::TreeKey;
use crate::places::Place;
use crate::settings::{Favorite, Settings};
use crate::state::{RecentAt, Unkept};

/// A well-known folder of the system as the sidebar lists it under 즐겨찾기.
fn known(label: &str, name: &str) -> Place {
    Place {
        label: label.to_owned(),
        path: folder(name),
        kind: PlaceKind::Favorite(KnownFolder::Documents),
        space: None,
    }
}

/// A state whose sidebar lists the known folders 문서 and 사진 and the drive 디스크.
fn with_places() -> AppState {
    let mut state = two_panes();
    state.set_places(&[
        known("문서", "docs-known"),
        known("사진", "pics-known"),
        drive("디스크", &temp()),
    ]);
    state
}

/// The labels of the places the sidebar lists, in order.
fn place_labels(state: &AppState) -> Vec<String> {
    state
        .places()
        .iter()
        .map(|place| place.label.clone())
        .collect()
}

/// The labels of the roots of the sidebar's tree, in order (the tree is closed).
fn root_labels(state: &AppState) -> Vec<String> {
    state
        .tree()
        .rows()
        .into_iter()
        .filter(|row| row.root.is_some())
        .map(|row| row.label)
        .collect()
}

fn names(state: &AppState) -> Vec<String> {
    state
        .favorites()
        .iter()
        .map(|favorite| favorite.name.clone())
        .collect()
}

/// Whether `effects` ask the job engine for anything: a drop on the list of favorites moves,
/// copies and links nothing.
fn asks_for_a_job(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Job(_)))
}

fn both() -> Effects {
    Effects::COPY | Effects::MOVE
}

fn plain() -> DragMods {
    DragMods::default()
}

#[test]
fn the_first_edit_makes_the_known_folders_the_users_own_list() {
    let mut state = with_places();
    // Nothing is kept yet: the sidebar shows the system's folders as before.
    assert_eq!(state.favorites(), []);
    assert_eq!(place_labels(&state), ["문서", "사진", "디스크"]);

    let work = folder("work");
    let effects = state.favorite_add(&work);

    assert!(!asks_for_a_job(&effects));
    assert_eq!(
        state.favorites(),
        [
            Favorite::new("문서", &folder("docs-known")),
            Favorite::new("사진", &folder("pics-known")),
            Favorite::new("work", &work),
        ]
    );
    // The sidebar lists them, and its tree starts again from them.
    assert_eq!(place_labels(&state), ["문서", "사진", "work", "디스크"]);
    assert_eq!(root_labels(&state), ["문서", "사진", "work", "디스크"]);
    assert_eq!(state.places()[2].kind, PlaceKind::Kept);

    // A folder that is there already is not added twice.
    let effects = state.favorite_add(&work);
    assert_eq!(state.favorites().len(), 3);
    assert!(
        matches!(effects.as_slice(), [Effect::Notice(_)]),
        "{effects:?}"
    );
}

#[test]
fn favorites_are_removed_renamed_and_moved_and_the_list_is_the_users() {
    let mut state = with_places();

    // Removing from the known folders keeps the others, as the user's list.
    state.favorite_remove(0);
    assert_eq!(names(&state), ["사진"]);
    assert_eq!(place_labels(&state), ["사진", "디스크"]);
    // A new favorite is not touched when the list of the known folders is found again.
    state.favorite_add(&folder("a1"));
    state.favorite_add(&folder("b1"));
    state.set_places(&[known("문서", "docs-known"), drive("디스크", &temp())]);
    assert_eq!(names(&state), ["사진", "a1", "b1"]);
    assert_eq!(place_labels(&state), ["사진", "a1", "b1", "디스크"]);

    state.favorite_rename(1, "  첫째  ");
    assert_eq!(names(&state), ["사진", "첫째", "b1"]);
    assert_eq!(root_labels(&state), ["사진", "첫째", "b1", "디스크"]);
    // A name of nothing but spaces changes nothing.
    state.favorite_rename(1, "   ");
    assert_eq!(names(&state), ["사진", "첫째", "b1"]);

    // Moves: the final position is given; the ends stop it.
    state.favorite_move(2, 0);
    assert_eq!(names(&state), ["b1", "사진", "첫째"]);
    state.favorite_move(0, 1);
    assert_eq!(names(&state), ["사진", "b1", "첫째"]);
    state.favorite_move(0, 9);
    assert_eq!(names(&state), ["b1", "첫째", "사진"]);
    state.favorite_move(7, 0);
    state.favorite_remove(9);
    state.favorite_rename(9, "x");
    assert_eq!(names(&state), ["b1", "첫째", "사진"]);

    // The last one can go: the list is then the user's, empty, not the system's again.
    for _ in 0..3 {
        state.favorite_remove(0);
    }
    assert_eq!(place_labels(&state), ["디스크"]);
    assert_eq!(state.favorites(), []);
}

#[test]
fn the_users_favorites_and_recent_folders_are_kept_in_the_settings_and_come_back() {
    let mut state = with_places();
    state.favorite_add(&folder("work"));
    state.favorite_rename(0, "내 문서");
    state.favorite_move(2, 0);
    state.set_recent_folders(vec![folder("r1"), folder("r2")]);
    state.set_recent_folded(true);
    let mut settings = Settings::default();
    state.save_sidebar(&mut settings);

    // Through the file and back: the list in its order, under the names the user gave.
    let dir = tempfile::tempdir().expect("a temporary folder");
    settings.save(dir.path()).expect("saved");
    let loaded = crate::settings::load(dir.path()).settings;
    let mut next = with_places();
    next.restore_sidebar(&loaded);
    assert_eq!(names(&next), ["work", "내 문서", "사진"]);
    assert_eq!(place_labels(&next), ["work", "내 문서", "사진", "디스크"]);
    assert_eq!(root_labels(&next), ["work", "내 문서", "사진", "디스크"]);
    assert_eq!(next.recent_folders(), [folder("r1"), folder("r2")]);
    assert!(next.recent_folded());

    // Nothing is kept of a list the user never edited, and a list they emptied stays so.
    let mut untouched = Settings::default();
    with_places().save_sidebar(&mut untouched);
    assert_eq!(
        (untouched.favorites.len(), untouched.favorites_emptied),
        (0, false)
    );
    let mut emptied = with_places();
    emptied.favorite_remove(0);
    emptied.favorite_remove(0);
    let mut settings = Settings::default();
    emptied.save_sidebar(&mut settings);
    assert_eq!(
        (settings.favorites.len(), settings.favorites_emptied),
        (0, true)
    );
    let mut again = with_places();
    again.restore_sidebar(&settings);
    assert_eq!(place_labels(&again), ["디스크"]);
}

#[test]
fn an_emptied_list_stays_empty_until_the_user_adds_again() {
    let mut state = with_places();
    state.favorite_remove(0);
    state.favorite_remove(0);
    assert_eq!(place_labels(&state), ["디스크"]);
    // The settings keep no list for an empty one, so the system's folders show at the next
    // start: the state says whether the user has edited at all.
    assert!(state.favorites_edited());
    state.favorite_add(&folder("x"));
    assert_eq!(place_labels(&state), ["x", "디스크"]);
}

#[test]
fn the_row_menu_opens_moves_and_renames() {
    let mut state = with_places();
    state.favorite_add(&folder("work"));
    // 위로 and 아래로 move the row one place.
    state.favorite_command(2, "up");
    assert_eq!(names(&state), ["문서", "work", "사진"]);
    state.favorite_command(1, "down");
    assert_eq!(names(&state), ["문서", "사진", "work"]);
    state.favorite_command(2, "down");
    state.favorite_command(0, "up");
    assert_eq!(names(&state), ["문서", "사진", "work"]);
    // 열기 shows the folder in the active pane, 반대 패널에 열기 in the other one.
    let active = state.active();
    let effects = state.favorite_command(2, "open");
    assert_eq!(the_load(&effects).path, folder("work"));
    assert_eq!(
        state.panes()[active].active_tab().location(),
        &folder("work")
    );
    let other = 1 - active;
    let effects = state.favorite_command(1, "open-other");
    assert_eq!(the_load(&effects).path, folder("pics-known"));
    assert_eq!(
        state.panes()[other].active_tab().location(),
        &folder("pics-known")
    );
    assert_eq!(state.active(), active);
    // 이름 바꾸기 asks for the name in the name window; the name it is confirmed with is kept.
    state.favorite_command(1, "rename");
    let Some(Modal::Name(dialog)) = state.modal() else {
        panic!("the name window: {:?}", state.modal());
    };
    assert_eq!(dialog.purpose, NamePurpose::Favorite { index: 1 });
    assert_eq!(dialog.initial, "사진");
    let effects = state.name_confirmed("풍경");
    assert!(!asks_for_a_job(&effects));
    assert_eq!(names(&state), ["문서", "풍경", "work"]);
    assert!(state.modal().is_none());
    // 즐겨찾기에서 제거 removes without asking.
    state.favorite_command(2, "remove");
    assert_eq!(names(&state), ["문서", "풍경"]);
    // 현재 폴더를 즐겨찾기에 추가 (the empty area's menu) keeps the folder the active pane shows.
    state.go_to(folder("work"));
    state.favorites_blank_command("add-current");
    assert_eq!(names(&state), ["문서", "풍경", "work"]);
}

#[test]
fn a_folder_dropped_between_favorites_is_added_and_nothing_moves() {
    let mut state = with_places();
    // The left pane's folders `docs` and `src` are dragged: the press readies the drag.
    state.press_row(LEFT, 0, false, false);
    state.release_row(LEFT, 0);
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    assert_ne!(id, 0);

    // Between the two favorites, the pointer shows an addition, and a drop adds the folder
    // there: no job is asked for, and the panes' rows stay as they are.
    let between = DropSpot::Favorites { index: Some(1) };
    assert_eq!(
        state.drop_hover(id, between),
        Hover::Keeps {
            index: Some(1),
            count: 1
        }
    );
    let effects = state.drop_items(id, between);
    assert!(!asks_for_a_job(&effects), "{effects:?}");
    assert_eq!(names(&state), ["문서", "docs", "사진"]);
    assert_eq!(
        state.favorites()[1].folder(),
        Some(folder("a").join("docs").unwrap())
    );
    assert_eq!(rows(&state, LEFT), ["docs", "src", "x.txt"]);
    assert_eq!(root_labels(&state), ["문서", "docs", "사진", "디스크"]);
    // The drag is over: a second drop of it adds nothing.
    assert_eq!(state.drag_id(), 0);

    // The end of the list takes a folder too (the empty area), and a folder that is there
    // moves to the place of the drop rather than twice into the list.
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    let effects = state.drop_items(id, DropSpot::Favorites { index: None });
    assert!(!asks_for_a_job(&effects));
    assert_eq!(names(&state), ["문서", "사진", "docs"]);
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    state.drop_items(id, DropSpot::Favorites { index: Some(0) });
    assert_eq!(names(&state), ["docs", "문서", "사진"]);
    // Dropped right where it is, nothing changes.
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    state.drop_items(id, DropSpot::Favorites { index: Some(1) });
    assert_eq!(names(&state), ["docs", "문서", "사진"]);

    // Several folders go in order from the place of the drop.
    state.press_row(LEFT, 0, false, false);
    state.release_row(LEFT, 0);
    state.press_row(LEFT, 1, false, true);
    assert_eq!(selected(&state, LEFT), [0, 1]);
    let id = state.drag_id();
    assert_eq!(
        state.drop_hover(id, DropSpot::Favorites { index: Some(2) }),
        Hover::Keeps {
            index: Some(2),
            count: 2
        }
    );
    state.drop_items(id, DropSpot::Favorites { index: Some(2) });
    assert_eq!(names(&state), ["문서", "docs", "src", "사진"]);
}

#[test]
fn files_dragged_between_favorites_are_refused_and_so_is_a_link_key() {
    let mut state = with_places();
    // `x.txt` is a file: the list takes folders only.
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    let between = DropSpot::Favorites { index: Some(1) };
    assert_eq!(state.drop_hover(id, between), Hover::NoKeep(Unkept::Files));
    assert_eq!(state.drop_items(id, between), []);
    assert_eq!(state.favorites(), []);
    // A file among folders refuses the whole drop.
    state.press_row(LEFT, 0, false, false);
    state.release_row(LEFT, 0);
    state.press_row(LEFT, 2, false, true);
    let id = state.drag_id();
    assert_eq!(state.drop_hover(id, between), Hover::NoKeep(Unkept::Files));
    assert_eq!(state.drop_items(id, between), []);
    assert_eq!(state.favorites(), []);

    // The key for a link: the list takes folders only as additions, and links are not made.
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    state.modifiers_changed(link_keys());
    assert_eq!(state.drop_hover(id, between), Hover::NoKeep(Unkept::Link));
    assert_eq!(state.drop_items(id, between), []);
    assert_eq!(state.favorites(), []);
    state.modifiers_changed(plain());
    // The payload of another drag takes nothing there.
    assert_eq!(
        state.drop_hover(id.wrapping_add(1), between),
        Hover::Nothing
    );
}

#[test]
fn a_favorite_takes_a_drop_onto_it_as_a_folder_does() {
    let mut state = with_places();
    state.favorite_add(&folder("work"));
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // Onto the row of a favorite: the existing rule, the items go into its folder.
    let offer = state
        .drop_offer(id, DropSpot::Tree(2))
        .expect("the favorite takes the items");
    assert_eq!(offer.folder, folder("work"));
    let effects = state.drop_items(id, DropSpot::Tree(2));
    let (spec, _) = the_submit(&effects);
    assert!(matches!(spec, JobSpec::Transfer(_)));
}

/// A folder of the disk, which another app can drag: a drag of its path tells nothing of
/// whether it is a folder.
struct Dragged {
    _dir: tempfile::TempDir,
    folders: Vec<PathBuf>,
    file: PathBuf,
}

fn dragged() -> Dragged {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let folders: Vec<PathBuf> = ["프로젝트", "자료", "보관"]
        .iter()
        .map(|name| {
            let path = dir.path().join(name);
            std::fs::create_dir(&path).expect("a folder");
            path
        })
        .collect();
    let file = dir.path().join("메모.txt");
    std::fs::write(&file, b"x").expect("a file");
    Dragged {
        _dir: dir,
        folders,
        file,
    }
}

fn items(paths: &[&PathBuf]) -> DroppedItems {
    DroppedItems {
        paths: paths.iter().map(|path| (*path).clone()).collect(),
        ..DroppedItems::default()
    }
}

/// What the controller's worker does with the effect of a drop of another app's paths on the list
/// ([`Effect::KeepDropped`]): looks at which are folders and tells the state. `None` when the
/// effects hold no such request.
fn worker_keeps(state: &mut AppState, effects: &[Effect]) -> Option<Vec<Effect>> {
    let Some(Effect::KeepDropped {
        generation,
        sources,
        index,
    }) = effects
        .iter()
        .find(|effect| matches!(effect, Effect::KeepDropped { .. }))
    else {
        return None;
    };
    let (folders, others): (Vec<VPath>, Vec<VPath>) = sources.iter().cloned().partition(|path| {
        path.as_local()
            .is_some_and(|path| std::fs::metadata(path).is_ok_and(|meta| meta.is_dir()))
    });
    Some(state.folders_kept(*generation, folders, !others.is_empty(), *index))
}

#[test]
fn a_folder_dragged_from_another_app_between_favorites_is_added_and_nothing_moves() {
    let mut state = with_places();
    let from = dragged();
    let between = DropSpot::Favorites { index: Some(1) };
    let one = items(&[&from.folders[0]]);

    // The pointer shows an addition (a link where the drag allows one, else a copy: never a
    // move, which the other app would finish by deleting the folder).
    assert_eq!(
        state.outside_hover(Some(between), &one, plain(), both()),
        Hover::Keeps {
            index: Some(1),
            count: 1
        }
    );
    assert_eq!(
        state.outside_over(Some(between), &one, plain(), both()),
        Some(DropEffect::Copy)
    );
    assert_eq!(
        state.outside_over(Some(between), &one, plain(), Effects::ALL),
        Some(DropEffect::Link)
    );
    let (done, effects) = state.outside_drop(Some(between), one, plain(), both(), false);
    // Nothing was moved or copied: the other app is told nothing was done. The folders are told
    // by a worker, which the list waits for.
    assert_eq!(done, None);
    assert!(!asks_for_a_job(&effects), "{effects:?}");
    assert!(!state.favorites_edited());
    let kept = worker_keeps(&mut state, &effects).expect("a worker looks at the paths");
    assert!(!asks_for_a_job(&kept), "{kept:?}");
    assert_eq!(names(&state), ["문서", "프로젝트", "사진"]);

    // Two folders, at the end; one of them is there already and moves to the end.
    let two = items(&[&from.folders[1], &from.folders[0]]);
    let (done, effects) = state.outside_drop(
        Some(DropSpot::Favorites { index: None }),
        two,
        plain(),
        both(),
        false,
    );
    assert_eq!((done, asks_for_a_job(&effects)), (None, false));
    worker_keeps(&mut state, &effects).expect("a worker looks at the paths");
    assert_eq!(names(&state), ["문서", "사진", "자료", "프로젝트"]);

    // A file: the hover cannot tell (nothing is looked at), the worker keeps no folder and a
    // notice says why; a link key is refused at once (and a right-button drop adds as well).
    let file = items(&[&from.file]);
    let (done, effects) = state.outside_drop(Some(between), file, plain(), both(), false);
    assert_eq!(done, None);
    let told = worker_keeps(&mut state, &effects).expect("a worker looks at the paths");
    assert_eq!(
        told,
        [Effect::Notice(words::drop_only_folders().to_owned())]
    );
    // A mix keeps its folders and tells of the file.
    let mix = items(&[&from.file, &from.folders[2]]);
    let (_, effects) = state.outside_drop(
        Some(DropSpot::Favorites { index: Some(0) }),
        mix,
        plain(),
        both(),
        false,
    );
    let told = worker_keeps(&mut state, &effects).expect("a worker looks at the paths");
    assert_eq!(
        told.last(),
        Some(&Effect::Notice(words::drop_only_folders().to_owned()))
    );
    assert_eq!(
        names(&state)[0],
        from.folders[2].file_name().unwrap().to_string_lossy()
    );
    state.favorite_remove(0);
    let folder_drag = items(&[&from.folders[0]]);
    assert_eq!(
        state.outside_hover(Some(between), &folder_drag, link_keys(), both()),
        Hover::NoKeep(Unkept::Link)
    );
    let (done, effects) =
        state.outside_drop(Some(between), folder_drag, link_keys(), both(), false);
    assert_eq!((done, effects), (None, Vec::new()));
    assert_eq!(names(&state), ["문서", "사진", "자료", "프로젝트"]);
    assert_eq!(rows(&state, LEFT), ["docs", "src", "x.txt"]);

    // The list was set from the settings while the worker looked: its answer is stale.
    let again = items(&[&from.folders[2]]);
    let (_, effects) = state.outside_drop(Some(between), again, plain(), both(), false);
    state.set_favorites(Vec::new(), false);
    assert_eq!(worker_keeps(&mut state, &effects), Some(Vec::new()));
    assert_eq!(state.favorites(), []);
}

#[test]
fn hovering_and_dropping_on_the_list_never_look_at_the_disk() {
    let mut state = with_places();
    // Paths that exist nowhere: a look at the disk would find no folder and refuse them.
    let nowhere = items(&[&temp().as_local().expect("local").join("no/such/folder")]);
    let between = DropSpot::Favorites { index: Some(1) };
    for spot in [between, DropSpot::Favorites { index: None }] {
        assert!(matches!(
            state.outside_hover(Some(spot), &nowhere, plain(), both()),
            Hover::Keeps { count: 1, .. }
        ));
        assert_eq!(
            state.outside_over(Some(spot), &nowhere, plain(), both()),
            Some(DropEffect::Copy)
        );
    }
    let (done, effects) = state.outside_drop(Some(between), nowhere, plain(), both(), false);
    assert_eq!(done, None);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::KeepDropped { index: Some(1), .. }]
        ),
        "{effects:?}"
    );
    // Nothing is kept until a worker says there is a folder.
    assert!(!state.favorites_edited());
}

#[test]
fn recent_folders_are_the_last_ten_visited_latest_first_without_repeats() {
    let mut state = ready(vec![], vec![]);
    state.note_visits();
    // Both panes' folders were visited; the active pane's is the latest.
    assert_eq!(state.recent_folders(), [folder("a"), folder("b")]);

    // Every navigation of either pane brings its folder to the front, once.
    state.go_to(folder("c"));
    state.note_visits();
    state.activate(RIGHT);
    state.go_to(folder("d"));
    state.note_visits();
    assert_eq!(
        state.recent_folders(),
        [folder("d"), folder("c"), folder("a"), folder("b")]
    );
    // A folder visited again moves to the front rather than twice into the list; a tab that
    // shows the same folder is no new visit.
    state.go_to(folder("a"));
    state.note_visits();
    state.note_visits();
    assert_eq!(state.recent_folders()[0], folder("a"));
    assert_eq!(state.recent_folders().len(), 4);

    // Ten at most, the oldest dropping out; the folders of the favorites count as well.
    state.favorite_add(&folder("fav"));
    for i in 0..12 {
        state.go_to(folder(&format!("n{i}")));
        state.note_visits();
    }
    state.go_to(folder("fav"));
    state.note_visits();
    assert_eq!(state.recent_folders().len(), 10);
    assert_eq!(state.recent_folders()[0], folder("fav"));
    assert_eq!(state.recent_folders()[1], folder("n11"));
    assert_eq!(state.recent_folders()[9], folder("n3"));
}

#[test]
fn a_missing_recent_folder_dims_and_a_click_says_it_is_gone() {
    let mut state = ready(vec![], vec![]);
    state.note_visits();
    let gone = folder("gone");
    let here = folder("here");
    let effects = state.set_recent_folders(vec![gone.clone(), here.clone()]);
    // A worker looks which of them are still there.
    let Some(Effect::CheckFolders(asked)) = effects.first() else {
        panic!("a look at the folders: {effects:?}");
    };
    assert_eq!(asked, &[gone.clone(), here.clone()]);
    state.folders_checked(&[(gone.clone(), false), (here.clone(), true)]);
    assert!(state.is_missing(&gone));
    assert!(!state.is_missing(&here));

    // The click on the missing one shows the notice of a folder that cannot be found, and no
    // pane moves to it.
    let location = state.panes()[state.active()]
        .active_tab()
        .location()
        .clone();
    let effects = state.open_recent(0);
    assert_eq!(effects, [Effect::Notice(words::start_folder_gone(&gone))]);
    assert_eq!(
        state.panes()[state.active()].active_tab().location(),
        &location
    );
    // The other one opens in the active pane.
    let effects = state.open_recent(1);
    assert_eq!(the_load(&effects).path, here);
    // Visiting a folder again finds it there.
    state.go_to(gone.clone());
    state.note_visits();
    assert!(!state.is_missing(&gone));
}

#[test]
fn a_missing_favorite_dims_and_a_click_says_it_is_gone_and_the_look_repeats() {
    let mut state = with_places();
    state.note_visits();
    state.favorite_add(&folder("kept"));
    let start = Instant::now();
    // The first look asks about the recent folders and the favorites, each once.
    let effects = state.check_folders_due(start);
    let Some(Effect::CheckFolders(asked)) = effects.first() else {
        panic!("a look at the folders: {effects:?}");
    };
    assert!(asked.contains(&folder("kept")) && asked.contains(&folder("docs-known")));
    assert_eq!(asked.iter().collect::<HashSet<_>>().len(), asked.len());
    state.folders_checked(&[(folder("kept"), false), (folder("docs-known"), true)]);
    assert!(state.is_missing(&folder("kept")));
    assert!(!state.is_missing(&folder("docs-known")));

    // The click on the dimmed favorite says so, and no pane moves; so does its menu's 열기.
    let location = state.panes()[state.active()]
        .active_tab()
        .location()
        .clone();
    let row = state
        .tree()
        .rows()
        .iter()
        .position(|row| row.label == "kept")
        .expect("the favorite's row");
    assert_eq!(
        state.open_tree_row(row),
        [Effect::Notice(words::start_folder_gone(&folder("kept")))]
    );
    let index = state
        .favorites()
        .iter()
        .position(|favorite| favorite.folder() == Some(folder("kept")))
        .expect("a favorite");
    assert_eq!(
        state.favorite_command(index, "open"),
        [Effect::Notice(words::start_folder_gone(&folder("kept")))]
    );
    assert_eq!(
        state.panes()[state.active()].active_tab().location(),
        &location
    );

    // No new look within half a minute, one after it; it finds the folder back.
    let soon = start + Duration::from_secs(29);
    assert_eq!(state.check_folders_due(soon), Vec::<Effect>::new());
    let later = start + Duration::from_secs(31);
    assert!(matches!(
        state.check_folders_due(later).as_slice(),
        [Effect::CheckFolders(_)]
    ));
    state.folders_checked(&[(folder("kept"), true)]);
    assert!(!state.is_missing(&folder("kept")));
    assert_eq!(the_load(&state.open_tree_row(row)).path, folder("kept"));
}

#[test]
fn the_menu_rename_and_delete_keys_work_on_a_favorite_s_row() {
    let mut state = with_places();
    press(&mut state, &special(Key::F6), NONE);
    state.tree_key(TreeKey::Down);
    assert_eq!(state.tree_cursor_row(), Some(1));

    // The menu key opens the menu of the favorite, and on any other row the empty part's.
    assert_eq!(
        state.tree_key(TreeKey::Menu),
        [Effect::ShowMenu(MenuShown::Favorite { index: 1 })]
    );
    state.tree_key(TreeKey::Down);
    assert_eq!(state.tree_cursor_row(), Some(2));
    assert_eq!(
        state.tree_key(TreeKey::Menu),
        [Effect::ShowMenu(MenuShown::FavoritesBlank)]
    );
    // F2 renames a favorite and does nothing on a drive.
    assert_eq!(state.tree_key(TreeKey::Rename), []);
    assert!(state.modal().is_none());
    state.tree_key(TreeKey::Up);
    state.tree_key(TreeKey::Rename);
    assert!(matches!(
        state.modal(),
        Some(Modal::Name(dialog)) if dialog.purpose == NamePurpose::Favorite { index: 1 }
    ));
    state.dialog_cancelled();
    // Enter opens, but on macOS renames a favorite: the key says which, the row decides.
    state.tree_key(TreeKey::OpenOrRename);
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    state.dialog_cancelled();
    state.tree_key(TreeKey::Down);
    let effects = state.tree_key(TreeKey::OpenOrRename);
    assert!(state.modal().is_none());
    assert_eq!(the_load(&effects).path, temp());

    // Delete takes the favorite out without asking, and the cursor stays among the favorites.
    state.tree_key(TreeKey::Up);
    assert_eq!(state.tree_cursor_row(), Some(1));
    let effects = state.tree_key(TreeKey::Remove);
    assert!(!asks_for_a_job(&effects), "{effects:?}");
    assert_eq!(names(&state), ["문서"]);
    assert!(state.modal().is_none());
    assert_eq!(state.tree_cursor_row(), Some(0));
    // On a drive it removes nothing.
    state.tree_key(TreeKey::Down);
    assert_eq!(state.tree_cursor_row(), Some(1));
    assert_eq!(state.tree_key(TreeKey::Remove), []);
    assert_eq!(names(&state), ["문서"]);
}

#[test]
fn the_arrows_walk_through_the_group_of_recent_folders_and_fold_it() {
    let mut state = with_places();
    let (r1, r2) = (folder("r1"), folder("r2"));
    state.set_recent_folders(vec![r1.clone(), r2.clone()]);
    // The group lies between the favorites and the drives.
    assert_eq!(state.recent_slot(), Some(2));
    press(&mut state, &special(Key::F6), NONE);
    state.tree_key(TreeKey::Down);
    assert_eq!(state.tree_cursor_row(), Some(1));

    // ↓ from the last favorite goes into the group: its head, then its folders, then the drive.
    assert_eq!(
        state.tree_key(TreeKey::Down),
        [Effect::ScrollRecent(RecentAt::Head)]
    );
    assert_eq!(
        (state.tree_cursor_row(), state.recent_cursor()),
        (None, Some(RecentAt::Head))
    );
    state.tree_key(TreeKey::Down);
    assert_eq!(state.recent_cursor(), Some(RecentAt::Item(0)));
    state.tree_key(TreeKey::Down);
    assert_eq!(state.recent_cursor(), Some(RecentAt::Item(1)));
    assert_eq!(state.tree_key(TreeKey::Down), [Effect::ScrollTree(2)]);
    assert_eq!(
        (state.tree_cursor_row(), state.recent_cursor()),
        (Some(2), None)
    );
    // ↑ goes back the same way, to the last folder of the group first.
    state.tree_key(TreeKey::Up);
    assert_eq!(state.recent_cursor(), Some(RecentAt::Item(1)));
    state.tree_key(TreeKey::Up);
    state.tree_key(TreeKey::Up);
    assert_eq!(state.recent_cursor(), Some(RecentAt::Head));
    state.tree_key(TreeKey::Up);
    assert_eq!(state.tree_cursor_row(), Some(1));

    // ← on a folder goes to the head, ← on the head folds the group, → opens it again.
    state.tree_key(TreeKey::Down);
    state.tree_key(TreeKey::Down);
    state.tree_key(TreeKey::Collapse);
    assert_eq!(state.recent_cursor(), Some(RecentAt::Head));
    state.tree_key(TreeKey::Collapse);
    assert!(state.recent_folded());
    // Folded, the head is the group's only stop: ↓ goes on to the drive.
    state.tree_key(TreeKey::Down);
    assert_eq!(state.tree_cursor_row(), Some(2));
    state.tree_key(TreeKey::Up);
    assert_eq!(state.recent_cursor(), Some(RecentAt::Head));
    state.tree_key(TreeKey::Expand);
    assert!(!state.recent_folded());
    // Enter on the head folds it too; on a folder it shows the folder in the active pane.
    state.tree_key(TreeKey::Open);
    assert!(state.recent_folded());
    state.tree_key(TreeKey::Open);
    assert!(!state.recent_folded());
    state.tree_key(TreeKey::Expand);
    let effects = state.tree_key(TreeKey::Open);
    assert_eq!(the_load(&effects).path, r1);
    assert_eq!(state.recent_cursor(), Some(RecentAt::Item(0)));
    // The menu key in the group is the empty part's menu; F2 and Delete do nothing there.
    assert_eq!(
        state.tree_key(TreeKey::Menu),
        [Effect::ShowMenu(MenuShown::FavoritesBlank)]
    );
    assert_eq!(state.tree_key(TreeKey::Rename), []);
    assert_eq!(state.tree_key(TreeKey::Remove), []);
    // F6 gives the keys to a row of the tree again.
    state.tree_key(TreeKey::Leave);
    press(&mut state, &special(Key::F6), NONE);
    assert_eq!(state.recent_cursor(), None);
    assert!(state.tree_cursor_row().is_some());
}

#[test]
fn the_group_of_recent_folders_folds() {
    let mut state = with_places();
    assert!(!state.recent_folded());
    state.toggle_recent_fold();
    assert!(state.recent_folded());
    state.set_recent_folded(false);
    assert!(!state.recent_folded());
}

#[test]
fn only_one_look_at_the_folders_runs_and_the_one_asked_meanwhile_follows_its_answer() {
    let mut state = with_places();
    state.note_visits();
    state.favorite_add(&folder("kept"));
    let start = Instant::now();
    assert!(matches!(
        state.check_folders_due(start).as_slice(),
        [Effect::CheckFolders(_)]
    ));
    // The share does not answer: no look starts behind it, however long it takes or however
    // often the list is edited.
    let later = start + Duration::from_secs(100);
    assert_eq!(state.check_folders_due(later), Vec::<Effect>::new());
    assert_eq!(
        state.set_recent_folders(vec![folder("a")]),
        Vec::<Effect>::new()
    );
    // Its answer starts the one that was wanted meanwhile, and only that one.
    let next = state.folders_checked(&[(folder("kept"), false)]);
    assert!(matches!(next.as_slice(), [Effect::CheckFolders(_)]));
    assert_eq!(state.folders_checked(&[]), Vec::<Effect>::new());
    // With none wanted meanwhile, an answer starts nothing.
    assert!(matches!(
        state
            .check_folders_due(later + Duration::from_secs(40))
            .as_slice(),
        [Effect::CheckFolders(_)]
    ));
    assert_eq!(state.folders_checked(&[]), Vec::<Effect>::new());
}
