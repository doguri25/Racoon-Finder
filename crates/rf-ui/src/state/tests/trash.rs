//! 휴지통 and 내 PC in the panes (NAV-16): their rows, the address bar, what their commands do
//! (and refuse, saying why), the questions before a delete for good, restoring, and listing the
//! trash again.

use std::ffi::OsString;
use std::time::{Duration, SystemTime};

use rf_jobs::{
    Conflict, DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobSpec, Outcome,
    Stage, TrashedItem,
};
use rf_platform::DiskSpace;
use rf_vfs::{ShellPlace, TrashItem, trash_entry};

use super::jobs::finish;
use super::*;
use crate::dialogs::{ConfirmPurpose, ConflictChoice, ConflictQuestion, Modal};
use crate::format::{folder_label, location_text};
use crate::pick::PickPurpose;
use crate::places::{Place, shell_places};
use crate::words::JobWork;

fn trash() -> VPath {
    ShellPlace::Trash.path()
}

fn computer() -> VPath {
    ShellPlace::Computer.path()
}

/// An item of the trash called `name`, as a freedesktop trash in a temporary folder keeps it:
/// trashed from `from` (none: the trash does not keep where), of `size` bytes, at second 1000.
fn item(name: &str, from: Option<&str>, size: Option<u64>) -> TrashItem {
    let mut item = TrashItem::at(
        std::env::temp_dir().join("Trash").join("files").join(name),
        false,
    );
    item.original = from.map(|from| std::env::temp_dir().join(from).join(name));
    item.size = size;
    item.deleted = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000));
    item
}

/// The state with the left pane at the trash, which lists `items` (the right pane at `b`).
fn in_trash(items: &[TrashItem]) -> AppState {
    let mut state = ready(vec![], vec![]);
    let request = go(&mut state, trash());
    assert_eq!(request.path, trash());
    list_trash(&mut state, &request, items);
    state
}

/// The active pane goes to `place`, in the view it has there (as the controller settles it):
/// the request of its listing.
fn go(state: &mut AppState, place: VPath) -> LoadRequest {
    let mut effects = state.go_to(place);
    state.settle_views(&mut effects);
    the_load(&effects)
}

/// The trash's listing for `request`, as the loader sends it: its records, then its rows in
/// the order asked for.
fn list_trash(state: &mut AppState, request: &LoadRequest, items: &[TrashItem]) -> Vec<Effect> {
    let generation = request.generation;
    let mut effects = state.loaded(LoadEvent::TrashItems {
        generation,
        items: items.to_vec(),
    });
    let mut entries: Vec<Entry> = items.iter().map(trash_entry).collect();
    rf_vfs::sort_entries(&mut entries, &request.sort);
    effects.extend(state.loaded(LoadEvent::Listed {
        generation,
        entries,
    }));
    effects
}

/// The names the left pane's rows show.
fn shown(state: &AppState) -> Vec<String> {
    state.panes()[LEFT]
        .active_tab()
        .visible()
        .iter()
        .map(|entry| entry.shown_name().to_string_lossy().into_owned())
        .collect()
}

/// Selects the left pane's row that shows `name`.
fn choose(state: &mut AppState, name: &str) {
    let row = shown(state)
        .iter()
        .position(|shown| shown == name)
        .unwrap_or_else(|| panic!("{name} is a row"));
    state.click_row(LEFT, row, false, false);
}

/// The notices among `effects`.
fn notices(effects: &[Effect]) -> Vec<&str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notice(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The specs of the jobs `effects` submit.
fn submitted(effects: &[Effect]) -> Vec<&JobSpec> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Job(JobEffect::Submit { spec, .. }) => Some(spec),
            _ => None,
        })
        .collect()
}

fn confirm(state: &AppState) -> &crate::dialogs::Confirm {
    match state.modal() {
        Some(Modal::Confirm(confirm)) => confirm,
        other => panic!("a question: {other:?}"),
    }
}

#[test]
fn the_trash_shows_its_items_by_the_names_they_had_and_keeps_their_records() {
    let items = [
        item("b.txt", Some("docs"), Some(10)),
        item("a.txt", Some("docs"), Some(5)),
        item("note.md", None, None),
    ];
    let state = in_trash(&items);
    // By the names they had, in natural order: not by their ids.
    assert_eq!(shown(&state), ["a.txt", "b.txt", "note.md"]);
    let tab = state.panes()[LEFT].active_tab();
    for entry in tab.visible() {
        let record = tab
            .trash_item(&entry.name)
            .expect("each row has its record");
        assert_eq!(record.name, entry.shown_name());
        assert_eq!(OsString::from(record.id.as_str()), entry.name);
    }
    // The trash's own view: 원래 위치, 삭제한 날짜 and 크기 after the name.
    let columns: Vec<Column> = tab
        .view()
        .columns
        .iter()
        .map(|shown| shown.column)
        .collect();
    assert_eq!(columns, [Column::Original, Column::Deleted, Column::Size]);
    assert_eq!(Column::offered(&trash()), &Column::TRASH);
    // Where an item came from is its folder.
    let a = &tab.visible()[0];
    assert_eq!(
        a.origin.as_deref(),
        Some(std::env::temp_dir().join("docs").as_path())
    );
    assert_eq!(a.deleted, items[1].deleted);
}

#[test]
fn the_trash_s_columns_sort_and_its_view_is_its_own() {
    let items = [
        item("a.txt", Some("zeta"), Some(5)),
        item("b.txt", Some("alpha"), Some(10)),
        item("c.txt", None, Some(1)),
    ];
    let mut state = in_trash(&items);
    // By the original place: an unknown one first, then alpha before zeta.
    state.sort(LEFT, SortKey::Origin);
    assert_eq!(shown(&state), ["c.txt", "b.txt", "a.txt"]);
    state.sort(LEFT, SortKey::Size);
    assert_eq!(shown(&state), ["c.txt", "a.txt", "b.txt"]);
    // The folder remembers it, as any folder does.
    assert_eq!(
        state
            .views()
            .own(&trash())
            .map(|own| own.view.sort.primary.key),
        Some(SortKey::Size)
    );
    // A folder's columns are not the trash's, nor the trash's a folder's.
    state.toggle_column(LEFT, Column::Modified);
    assert!(
        !state.panes()[LEFT]
            .active_tab()
            .view()
            .shows(Column::Modified)
    );
    state.toggle_column(LEFT, Column::Deleted);
    assert!(
        !state.panes()[LEFT]
            .active_tab()
            .view()
            .shows(Column::Deleted)
    );
    state.toggle_column(RIGHT, Column::Original);
    assert!(
        !state.panes()[RIGHT]
            .active_tab()
            .view()
            .shows(Column::Original)
    );
}

#[test]
fn the_address_bar_names_the_places_and_takes_their_names_typed() {
    assert_eq!(location_text(&trash()), "휴지통");
    assert_eq!(location_text(&computer()), "내 PC");
    assert_eq!(folder_label(&trash()), "휴지통");
    assert_eq!(folder_label(&computer()), "내 PC");
    // A place has nothing above it: its path row is one piece, and Alt+↑ goes nowhere.
    assert_eq!(trash().parent(), None);
    assert_eq!(ancestors(&computer()), [computer()]);
    // An item of the trash is no place itself.
    let row = trash().join("x").expect("a name");
    assert_eq!(crate::format::shell_place_at(&row), None);
    let typed = |text: &str| read_typed(text, Os::Windows, &|_| None);
    for (text, place) in [
        ("휴지통", trash()),
        ("  휴지통 ", trash()),
        ("내 PC", computer()),
        ("내pc", computer()),
        ("shell://trash", trash()),
        ("SHELL://Computer", computer()),
    ] {
        assert_eq!(typed(text), Typed::Folder(place), "{text}");
    }
    // A path of the shell below a place is nothing the field opens.
    assert_eq!(typed("shell://trash/x"), Typed::Remote);

    // The field opens them, and the path row names them.
    let mut state = ready(vec![], vec![]);
    let effects = state.enter_path(LEFT, "휴지통");
    assert_eq!(the_load(&effects).path, trash());
    assert_eq!(state.panes()[LEFT].active_tab().location(), &trash());
}

#[test]
fn back_and_forward_go_through_the_places() {
    let mut state = ready(vec![], vec![]);
    state.go_to(trash());
    state.go_to(computer());
    state.go_to(folder("c"));
    let back = |state: &mut AppState| {
        press(state, &special(Key::LeftArrow), ALT);
        state.panes()[LEFT].active_tab().location().clone()
    };
    assert_eq!(back(&mut state), computer());
    assert_eq!(back(&mut state), trash());
    assert_eq!(back(&mut state), folder("a"));
    press(&mut state, &special(Key::RightArrow), ALT);
    assert_eq!(state.panes()[LEFT].active_tab().location(), &trash());
    // The places are no recent folders: the sidebar has them always.
    assert!(!state.recent_folders().contains(&trash()));
}

#[test]
fn what_writes_into_a_folder_is_refused_in_the_trash_with_a_notice() {
    use Command as C;
    let mut state = in_trash(&[item("a.txt", Some("docs"), Some(5))]);
    choose(&mut state, "a.txt");
    let refused = |state: &mut AppState, command: Command| {
        let effects = state
            .key(command.clone(), NONE, Paging::rows(10), Instant::now())
            .expect("the window takes it");
        assert_eq!(state.modal(), None, "{command:?}");
        assert!(submitted(&effects).is_empty(), "{command:?}");
        notices(&effects).first().map(|text| (*text).to_owned())
    };
    for (command, verb) in [
        (C::NewFolder, "새 폴더를 만들"),
        (C::Rename, "이름을 바꿀"),
        (C::Paste, "붙여넣을"),
        (C::Copy, "복사할"),
        (C::Cut, "잘라낼"),
        (C::CopyTo, "복사하거나 옮길"),
        (C::MoveTo, "복사하거나 옮길"),
        (C::CopyPath, "경로를 복사할"),
        (C::Properties, "속성을 볼"),
        (C::QuickLook, "미리 볼"),
        (C::OpenTerminal, "터미널을 열"),
        (C::OpenWith, "다른 프로그램으로 열"),
    ] {
        assert_eq!(
            refused(&mut state, command.clone()).as_deref(),
            Some(
                format!(
                    "휴지통에서는 {verb} 수 없습니다. 항목은 복원하거나 영구 삭제할 수 있습니다."
                )
                .as_str()
            ),
            "{command:?}"
        );
    }
    // An item of the trash is no file to open yet.
    assert_eq!(
        notices(&state.open_row(LEFT, 0)),
        ["휴지통 안의 항목은 열 수 없습니다. 먼저 복원하세요."]
    );
    // The menus' own ways, too.
    for effects in [
        state.new_file(),
        state.paste_from_menu(false),
        state.favorite_add_selected(),
        state.folder_properties(),
    ] {
        assert_eq!(notices(&effects).len(), 1, "{effects:?}");
        assert!(notices(&effects)[0].starts_with("휴지통에서는"));
    }
    assert_eq!(state.modal(), None);
    // No drag starts from its rows.
    state.press_row(LEFT, 0, false, false);
    assert_eq!(state.drag(), None);
    // Nor does the preview read an item of it.
    assert_eq!(state.follow_preview(), []);
}

#[test]
fn delete_in_the_trash_asks_first_and_only_its_button_deletes() {
    let items = [
        item("a.txt", Some("docs"), Some(1024)),
        item("b.txt", Some("docs"), Some(1024)),
    ];
    let mut state = in_trash(&items);
    state.key(Command::SelectAll, NONE, Paging::rows(10), Instant::now());
    // Delete, Shift+Delete and the menu's 영구 삭제… ask the same question.
    let asks: [fn(&mut AppState) -> Vec<Effect>; 3] = [
        |state| press(state, &special(Key::Delete), NONE).expect("taken"),
        |state| press(state, &special(Key::Delete), SHIFT).expect("taken"),
        |state| {
            state
                .trash_command("delete-from-trash")
                .expect("a command of the trash")
        },
    ];
    for ask in asks {
        let effects = ask(&mut state);
        assert_eq!(submitted(&effects), [] as [&JobSpec; 0]);
        let asked = confirm(&state);
        assert_eq!(asked.title, "영구 삭제");
        assert_eq!(
            asked.message,
            "휴지통의 항목 2개(2 KB)를 영구 삭제할까요?\n되돌릴 수 없습니다."
        );
        assert_eq!(asked.detail, "‘a.txt’, ‘b.txt’");
        assert_eq!(asked.ok, "영구 삭제");
        // Enter alone answers no: only the button deletes.
        assert!(asked.danger);
        // The answer that keeps them.
        assert_eq!(state.dialog_cancelled(), []);
    }
    assert_eq!(state.modal(), None);
    // Its button deletes exactly the items the list showed, with their records.
    state.trash_command("delete-from-trash");
    let effects = state.confirm_accepted();
    assert_eq!(
        submitted(&effects),
        [&JobSpec::DeleteFromTrash {
            items: vec![items[0].clone(), items[1].clone()]
        }]
    );
    // A size that is not known for every item is not said.
    let mut state = in_trash(&[item("a.txt", None, Some(5)), item("dir", None, None)]);
    state.key(Command::SelectAll, NONE, Paging::rows(10), Instant::now());
    state.trash_command("delete-from-trash");
    assert_eq!(
        confirm(&state).message,
        "휴지통의 항목 2개를 영구 삭제할까요?\n되돌릴 수 없습니다."
    );
}

#[test]
fn emptying_the_trash_reads_it_again_and_asks_with_what_it_holds_then() {
    let mut state = in_trash(&[item("a.txt", Some("docs"), Some(5))]);
    let effects = state
        .trash_command("empty-trash")
        .expect("a command of the trash");
    // No question before the trash is read again.
    assert_eq!(state.modal(), None);
    let request = the_load(&effects);
    assert_eq!(request.path, trash());
    // Meanwhile another app trashed one more, and the window's listing has it.
    let now = [
        item("a.txt", Some("docs"), Some(5)),
        item("c.txt", Some("docs"), Some(7)),
    ];
    assert_eq!(list_trash(&mut state, &request, &now), []);
    let asked = confirm(&state);
    assert_eq!(asked.title, "휴지통 비우기");
    assert_eq!(
        asked.message,
        "휴지통의 항목 2개(12 바이트)를 모두 영구 삭제할까요?\n되돌릴 수 없습니다."
    );
    assert_eq!(asked.ok, "휴지통 비우기");
    assert!(asked.danger);
    assert_eq!(asked.purpose, ConfirmPurpose::EmptyTrash(now.to_vec()));
    assert_eq!(state.dialog_cancelled(), []);
    // Yes empties it.
    let effects = state.trash_command("empty-trash").expect("a command");
    let request = the_load(&effects);
    list_trash(&mut state, &request, &now);
    let effects = state.confirm_accepted();
    // It deletes the items it counted, each while it is that very item.
    assert_eq!(
        submitted(&effects),
        [&JobSpec::DeleteFromTrash {
            items: now.to_vec()
        }]
    );

    // An empty trash asks nothing.
    let effects = state.trash_command("empty-trash").expect("a command");
    let request = the_load(&effects);
    let effects = list_trash(&mut state, &request, &[]);
    assert_eq!(notices(&effects), ["휴지통이 비어 있습니다"]);
    assert_eq!(state.modal(), None);
    // Outside the trash it does nothing.
    state.go_to(folder("c"));
    assert_eq!(state.trash_command("empty-trash"), Some(Vec::new()));
}

#[test]
fn restore_goes_to_the_original_place_and_an_unknown_one_offers_the_picker_alone() {
    let known = item("a.txt", Some("docs"), Some(5));
    let unknown = item("b.txt", None, Some(5));
    let mut state = in_trash(&[known.clone(), unknown.clone()]);
    choose(&mut state, "a.txt");
    state.row_menu(LEFT, 0);
    assert!(state.item_menu().can_restore);
    let effects = state
        .trash_command("restore")
        .expect("a command of the trash");
    let to = known.original.clone().expect("an original place");
    assert_eq!(
        submitted(&effects),
        [&JobSpec::RestoreFromTrash {
            items: vec![(known.clone(), to)]
        }]
    );

    // One the trash keeps no place for: 복원 is off, and says why when asked anyway.
    choose(&mut state, "b.txt");
    state.row_menu(LEFT, 1);
    assert!(!state.item_menu().can_restore);
    let effects = state.restore_items();
    assert_eq!(submitted(&effects), [] as [&JobSpec; 0]);
    assert_eq!(
        notices(&effects),
        ["원래 위치를 모르는 항목이 있어 복원할 수 없습니다. 위치를 골라 꺼내기…로 꺼내세요."]
    );
    // 위치를 골라 꺼내기…: the picker, which offers no shell place and no policy.
    let effects = state
        .trash_command("restore-elsewhere")
        .expect("a command of the trash");
    let (look, target) = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::InspectTarget {
                generation, path, ..
            } => Some((*generation, path.clone())),
            _ => None,
        })
        .expect("the picker looks at its first target");
    // The other pane's folder.
    assert_eq!(target, folder("b"));
    let Some(Modal::Pick(picker)) = state.modal() else {
        panic!("the picker shows: {:?}", state.modal());
    };
    assert_eq!(
        picker.purpose,
        PickPurpose::Restore {
            items: vec![unknown.clone()]
        }
    );
    assert!(picker.without_options());
    assert_eq!(
        crate::pick::validate(&picker.request, &trash(), None),
        crate::pick::Validity::ShellPlace
    );
    assert_eq!(
        crate::pick::validate(&picker.request, &computer(), None),
        crate::pick::Validity::ShellPlace
    );
    // It goes into the folder chosen, under the name it had.
    state.target_inspected(
        look,
        &target,
        crate::pick::TargetFacts {
            folder: Ok(()),
            writable: true,
            available: Some(1 << 30),
            same_volume: false,
            other_volume: false,
        },
    );
    let effects = state.pick_confirmed();
    let local = target.as_local().expect("a local folder").join("b.txt");
    assert_eq!(
        submitted(&effects),
        [&JobSpec::RestoreFromTrash {
            items: vec![(unknown, local)]
        }]
    );
}

/// What a worker finds at a folder that can take items, on the items' volume or on `other`.
fn found_folder(other: bool) -> crate::pick::TargetFacts {
    crate::pick::TargetFacts {
        folder: Ok(()),
        writable: true,
        available: Some(1 << 30),
        same_volume: !other,
        other_volume: other,
    }
}

#[test]
fn taking_items_out_refuses_a_folder_on_another_volume_up_front() {
    let a = item("a.txt", None, Some(5));
    let mut state = in_trash(std::slice::from_ref(&a));
    choose(&mut state, "a.txt");
    let effects = state
        .trash_command("restore-elsewhere")
        .expect("a command of the trash");
    let (look, target, from_dir) = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::InspectTarget {
                generation,
                path,
                from_dir,
            } => Some((*generation, path.clone(), from_dir.clone())),
            _ => None,
        })
        .expect("the picker looks at its first target");
    // The worker compares the target with the folder of the trash that holds the items: a
    // restore renames, which cannot cross volumes.
    let bin = a.location.parent().expect("the trash's folder");
    assert_eq!(from_dir, VPath::local(bin).expect("a local path"));
    // Nothing goes before the target was looked at.
    assert_eq!(state.pick_confirmed(), []);
    assert!(matches!(state.modal(), Some(Modal::Pick(_))));
    // On another volume: refused, and the picker says why.
    state.target_inspected(look, &target, found_folder(true));
    let Some(Modal::Pick(picker)) = state.modal() else {
        panic!("the picker shows: {:?}", state.modal());
    };
    let picker = picker.clone();
    assert_eq!(picker.validity(), Some(crate::pick::Validity::OtherVolume));
    assert!(!picker.can_confirm());
    assert_eq!(
        words::pick_problem(
            crate::pick::Validity::OtherVolume,
            TransferMode::Move,
            false
        ),
        "다른 드라이브라 꺼낼 수 없음: 휴지통의 항목은 같은 드라이브(볼륨)의 폴더로만 꺼냅니다"
    );
    assert_eq!(state.pick_confirmed(), []);
    // A copy or a move goes to another volume, though (it copies, then deletes).
    let mut moving = picker.clone();
    moving.purpose = PickPurpose::Transfer;
    assert_eq!(moving.validity(), Some(crate::pick::Validity::Ok));
    // On the items' volume, it goes.
    state.target_inspected(look, &target, found_folder(false));
    let effects = state.pick_confirmed();
    let local = target.as_local().expect("a local folder").join("a.txt");
    assert_eq!(
        submitted(&effects),
        [&JobSpec::RestoreFromTrash {
            items: vec![(a, local)]
        }]
    );
}

#[test]
fn items_of_two_trashes_are_not_taken_out_to_one_folder() {
    let a = item("a.txt", None, Some(5));
    let mut b = TrashItem::at(
        std::env::temp_dir()
            .join("Other volume")
            .join(".Trashes")
            .join("b.txt"),
        false,
    );
    b.size = Some(5);
    let mut state = in_trash(&[a, b]);
    state.click_row(LEFT, 0, false, false);
    state.click_row(LEFT, 1, true, false);
    let effects = state
        .trash_command("restore-elsewhere")
        .expect("a command of the trash");
    assert_eq!(state.modal(), None);
    assert_eq!(
        notices(&effects),
        [
            "고른 항목이 여러 드라이브(볼륨)의 휴지통에 있어 한 폴더로 꺼낼 수 없습니다. 드라이브마다 따로 골라 꺼내세요."
        ]
    );
}

#[test]
fn a_restore_s_conflict_never_offers_to_overwrite() {
    let item = item("a.txt", Some("docs"), Some(5));
    let path = |name: &str| folder("docs").join(name).expect("a name");
    let question = ConflictQuestion {
        id: 1,
        job: JobId(1),
        conflict: Box::new(Conflict {
            source: trash().join(item.id.as_str()).expect("an id"),
            target: path("a.txt"),
            incoming: file("a.txt"),
            existing: file("a.txt"),
        }),
        items_left: 0,
        work: JobWork::Restore,
    };
    assert!(!question.offers_overwrite());
    assert!(!question.can_overwrite());
    for choice in [
        ConflictChoice::Overwrite,
        ConflictChoice::OverwriteIfNewer,
        ConflictChoice::OverwriteIfSizeDiffers,
    ] {
        assert_eq!(question.answer(choice, false), None, "{choice:?}");
    }
    assert!(question.answer(ConflictChoice::Rename, true).is_some());
    assert!(question.answer(ConflictChoice::Skip, false).is_some());
    // A copy's still offers it.
    let copy = ConflictQuestion {
        work: JobWork::Copy,
        ..question
    };
    assert!(copy.offers_overwrite() && copy.can_overwrite());
}

#[test]
fn a_restore_says_where_the_items_went_and_its_undo_sends_them_back() {
    let a = item("a.txt", Some("docs"), Some(5));
    let mut state = in_trash(std::slice::from_ref(&a));
    let to = a.original.clone().expect("an original place");
    let spec = JobSpec::RestoreFromTrash {
        items: vec![(a.clone(), to.clone())],
    };
    let back = VPath::local(&to).expect("a local path");
    let record = UndoRecord::Restored {
        items: vec![TrashedItem {
            original: back.clone(),
            location: None,
            stamp: None,
        }],
    };
    let effects = finish(&mut state, 1, &spec, JobWork::Restore, |report| {
        report.outcome = Outcome::Completed;
        report.outputs = vec![back.clone()];
        report.undo = Some(record.clone());
    });
    // The trash is listed again.
    assert_eq!(the_load(&effects).path, trash());
    let toast = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Toast { text, action, .. } => Some((text.clone(), action.clone())),
            _ => None,
        })
        .expect("a toast");
    assert_eq!(
        toast,
        (
            "항목 1개를 ‘docs’ 폴더로 복원했습니다".to_owned(),
            "되돌리기".to_owned()
        )
    );
}

#[test]
fn a_restore_whose_folder_is_gone_offers_another_place() {
    let a = item("a.txt", Some("gone"), Some(5));
    let mut state = in_trash(std::slice::from_ref(&a));
    let to = a.original.clone().expect("an original place");
    let spec = JobSpec::RestoreFromTrash {
        items: vec![(a.clone(), to)],
    };
    let effects = finish(&mut state, 1, &spec, JobWork::Restore, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(ItemFailure {
            source: VPath::local(&a.location).ok(),
            target: None,
            stage: Stage::Restore,
            kind: ErrorKind::NotFound,
            message: String::new(),
            reason: Some(FailureReason::OriginalFolderGone),
        });
    });
    let token = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Toast {
                token,
                text,
                action,
                ..
            } => {
                assert_eq!(text, "원래 폴더가 없어 항목 1개를 복원하지 못했습니다");
                assert_eq!(action, "위치를 골라 꺼내기…");
                Some(*token)
            }
            _ => None,
        })
        .expect("a toast");
    state.toast_clicked(token);
    let Some(Modal::Pick(picker)) = state.modal() else {
        panic!("the picker shows: {:?}", state.modal());
    };
    assert_eq!(picker.purpose, PickPurpose::Restore { items: vec![a] });
}

#[test]
fn the_reasons_of_the_trash_s_jobs_are_said_in_words() {
    assert_eq!(
        words::failure_reason(&FailureReason::TrashItemChanged),
        "휴지통이 바뀌었습니다. 목록을 새로 고친 뒤 다시 해 주세요."
    );
    assert!(
        words::failure_reason(&FailureReason::OriginalFolderGone).contains("위치를 골라 꺼내기…")
    );
    assert_eq!(
        words::failure_reason(&FailureReason::RestoreNeverReplaces),
        "복원은 그 자리에 있는 항목을 덮어쓰지 않습니다"
    );
}

#[test]
fn the_trash_is_listed_again_after_its_jobs_and_when_the_window_comes_to_the_front() {
    let mut state = in_trash(&[item("a.txt", Some("docs"), Some(5))]);
    let before = state.panes()[LEFT].active_tab().generation();
    let effects = state.window_focused();
    let request = the_load(&effects);
    assert_eq!(request.path, trash());
    assert_ne!(request.generation, before);
    // The rows stay meanwhile.
    assert_eq!(shown(&state), ["a.txt"]);
    // A trash job of the app's, anywhere, lists it again; a copy does not.
    let trash_job = JobSpec::Delete(DeleteSpec {
        paths: vec![folder("b").join("x").expect("a name")],
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    });
    let effects = finish(&mut state, 1, &trash_job, JobWork::Trash, |report| {
        report.outcome = Outcome::Completed;
    });
    assert!(
        loads(&effects)
            .iter()
            .any(|request| request.path == trash())
    );
    assert!(changes_trash(&JobSpec::DeleteFromTrash {
        items: Vec::new()
    }));
    assert!(!changes_trash(&JobSpec::Create {
        parent: folder("b"),
        name: "c".into(),
        kind: rf_jobs::CreateKind::Dir,
    }));
    // F5 too, as anywhere.
    let effects = press(&mut state, &special(Key::F5), NONE).expect("taken");
    assert_eq!(the_load(&effects).path, trash());
}

/// A drive of the sidebar at the temporary folder's `name`.
fn drive(label: &str, name: &str, available: u64, total: u64) -> Place {
    Place {
        label: label.to_owned(),
        path: folder(name),
        kind: PlaceKind::Drive,
        space: Some(DiskSpace {
            total,
            free: available,
            available,
        }),
    }
}

#[test]
fn the_computer_lists_the_sidebar_s_drives_and_opens_one_at_its_top() {
    let mut state = ready(vec![], vec![]);
    let [my_pc, bin] = shell_places();
    state.set_places(&[my_pc, drive("시스템 디스크", "root", 50, 100), bin]);
    let request = go(&mut state, computer());
    assert_eq!(request.path, computer());
    state.list_computer(request.generation);
    assert_eq!(shown(&state), ["시스템 디스크"]);
    let tab = state.panes()[LEFT].active_tab();
    assert_eq!(tab.view().mode, ViewMode::Tiles);
    assert_eq!(tab.status(), LoadStatus::Ready);
    // Its view is its one: tiles.
    assert_eq!(
        notices(&state.set_view_mode(LEFT, ViewMode::Details)),
        ["내 PC는 드라이브를 타일로만 보여 줍니다"]
    );
    assert_eq!(
        state.panes()[LEFT].active_tab().view().mode,
        ViewMode::Tiles
    );
    // Nothing is made, deleted or pasted in it, and it says so.
    for command in [Command::NewFolder, Command::Trash, Command::Paste] {
        let effects = state
            .key(command, NONE, Paging::rows(10), Instant::now())
            .expect("taken");
        assert_eq!(notices(&effects).len(), 1);
        assert!(notices(&effects)[0].starts_with("내 PC에서는"));
    }
    // F5 finds the drives again.
    let effects = press(&mut state, &special(Key::F5), NONE).expect("taken");
    assert!(effects.contains(&Effect::FindPlaces));
    // Enter goes to the drive's top folder.
    state.click_row(LEFT, 0, false, false);
    let effects = state.open_row(LEFT, 0);
    assert_eq!(the_load(&effects).path, folder("root"));
    // 내 PC lists its drives again when they are found again.
    state.go_to(computer());
    let effects = state.set_places(&[drive("시스템 디스크", "root", 40, 100)]);
    assert!(
        loads(&effects)
            .iter()
            .any(|request| request.path == computer())
    );
}

#[test]
fn the_sidebar_has_the_places_and_a_drop_on_the_trash_sends_the_items_there() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let [my_pc, bin] = shell_places();
    let places = [
        Place {
            label: "홈".to_owned(),
            path: folder("home"),
            kind: PlaceKind::Favorite(rf_platform::KnownFolder::Home),
            space: None,
        },
        my_pc,
        drive("시스템 디스크", "root", 50, 100),
        bin,
    ];
    state.set_places(&places);
    let rows = state.tree().rows();
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, ["홈", "내 PC", "시스템 디스크", "휴지통"]);
    // The places of the shell have no folders below them: no chevron, never listed.
    for row in &rows[1..4] {
        let shell = state
            .tree()
            .node(row.id)
            .is_some_and(|node| !crate::tree::holds_folders(&node.path));
        if shell {
            assert!(!row.has_children_unknown && !row.has_children, "{row:?}");
        }
    }
    // Nor are they roots of the picker's tree.
    assert_eq!(state.pick_places().len(), 2);
    // A drag of a row onto 휴지통 trashes it, by the rules of Delete.
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    let spot = DropSpot::Tree { tree: 0, row: 3 };
    let offer = state.drop_offer(id, spot).expect("the trash takes it");
    assert_eq!(offer.folder, trash());
    let effects = state.drop_items(id, spot);
    assert_eq!(
        effects,
        [Effect::CheckTrash {
            folder: folder("a"),
            paths: vec![folder("a").join("a.txt").expect("a name")],
        }]
    );
    // On 내 PC, nothing.
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    let spot = DropSpot::Tree { tree: 0, row: 1 };
    assert_eq!(state.drop_offer(id, spot), None);
    assert_eq!(
        state.drop_refused(id, spot),
        Some(crate::pick::Validity::ShellPlace)
    );
    assert_eq!(
        notices(&state.drop_items(id, spot)),
        ["여기에 놓을 수 없습니다: 휴지통 목록과 내 PC에는 항목을 넣을 수 없습니다"]
    );
    // A place of the shell is not kept as a favorite.
    let effects = state.favorite_add(&trash());
    assert_eq!(notices(&effects).len(), 1);
}

#[test]
fn the_trash_and_the_computer_offer_what_their_capabilities_allow() {
    use rf_vfs::Capabilities;
    let bits = place_capabilities(&trash()).expect("a place of the shell");
    assert!(bits.contains(Capabilities::RESTORE | Capabilities::DELETE_FOREVER));
    assert!(!bits.intersects(Capabilities::WRITE | Capabilities::TRASH));
    assert_eq!(place_capabilities(&computer()), Some(Capabilities::empty()));
    assert_eq!(place_capabilities(&folder("a")), None);
    let entries = computer_entries(&[drive("USB", "usb", 1, 2), shell_places()[1].clone()]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].shown_name(), "USB");
    assert_eq!(entries[0].origin.as_deref(), folder("usb").as_local());
}

#[test]
fn emptying_deletes_exactly_what_its_question_counted() {
    let a = item("a.txt", Some("docs"), Some(5));
    let mut state = in_trash(std::slice::from_ref(&a));
    let effects = state.trash_command("empty-trash").expect("a command");
    let request = the_load(&effects);
    list_trash(&mut state, &request, std::slice::from_ref(&a));
    assert!(confirm(&state).message.contains("항목 1개"));
    // While the question is open the trash is read again (the window came to the front) and
    // holds c too; the question still counts a alone, and so does its answer.
    let c = item("c.txt", Some("docs"), Some(7));
    let effects = state.window_focused();
    let request = the_load(&effects);
    list_trash(&mut state, &request, &[a.clone(), c]);
    let effects = state.confirm_accepted();
    assert_eq!(
        submitted(&effects),
        [&JobSpec::DeleteFromTrash { items: vec![a] }]
    );
}

#[test]
fn the_retry_of_a_partly_failed_empty_deletes_only_what_failed_after_asking() {
    let (a, b) = (
        item("a.txt", Some("docs"), Some(5)),
        item("b.txt", Some("docs"), Some(5)),
    );
    let mut state = in_trash(&[a.clone(), b.clone()]);
    let spec = JobSpec::DeleteFromTrash {
        items: vec![a, b.clone()],
    };
    let effects = finish(&mut state, 1, &spec, JobWork::Delete, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(ItemFailure {
            source: VPath::local(&b.location).ok(),
            target: None,
            stage: Stage::Delete,
            kind: ErrorKind::PermissionDenied,
            message: String::new(),
            reason: None,
        });
    });
    let token = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Toast { token, .. } => Some(*token),
            _ => None,
        })
        .expect("a toast");
    state.toast_clicked(token);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items show: {:?}", state.modal());
    };
    assert_eq!(
        failures.retry.as_ref().map(|retry| &retry.spec),
        Some(&JobSpec::DeleteFromTrash {
            items: vec![b.clone()]
        })
    );
    assert_eq!(submitted(&state.failures_retried()), [] as [&JobSpec; 0]);
    assert_eq!(
        confirm(&state).purpose,
        ConfirmPurpose::RetryDeleteFromTrash {
            job: JobId(1),
            items: vec![b]
        }
    );
}

/// The failed item `b` of a delete from the trash, job 1, that deleted `a`.
fn a_trash_delete_that_failed_for(state: &mut AppState, a: &TrashItem, b: &TrashItem) {
    let spec = JobSpec::DeleteFromTrash {
        items: vec![a.clone(), b.clone()],
    };
    finish(state, 1, &spec, JobWork::Delete, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(ItemFailure {
            source: VPath::local(&b.location).ok(),
            target: None,
            stage: Stage::Delete,
            kind: ErrorKind::PermissionDenied,
            message: String::new(),
            reason: None,
        });
    });
}

/// Whether the window of failed items that shows offers 다시 시도.
fn offers_retry(state: &AppState) -> bool {
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items show: {:?}", state.modal());
    };
    failures.retry.is_some()
}

#[test]
fn cancelling_the_question_of_a_trash_retry_keeps_the_retry() {
    let (a, b) = (
        item("a.txt", Some("docs"), Some(5)),
        item("b.txt", Some("docs"), Some(5)),
    );
    let mut state = in_trash(&[a.clone(), b.clone()]);
    a_trash_delete_that_failed_for(&mut state, &a, &b);
    assert_eq!(state.recent_details(0), []);
    assert!(offers_retry(&state));
    // 다시 시도 asks first; cancelling that question (to look first) keeps the retry.
    assert_eq!(submitted(&state.failures_retried()), [] as [&JobSpec; 0]);
    assert!(confirm(&state).danger);
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);
    assert_eq!(state.recent_details(0), []);
    assert!(offers_retry(&state), "the retry is not used up by a cancel");
    // Asked again and answered yes: the failed item alone is deleted, and the retry is used up.
    assert_eq!(submitted(&state.failures_retried()), [] as [&JobSpec; 0]);
    assert_eq!(
        submitted(&state.confirm_accepted()),
        [&JobSpec::DeleteFromTrash {
            items: vec![b.clone()]
        }]
    );
    assert_eq!(state.recent_details(0), []);
    assert!(
        !offers_retry(&state),
        "a retry that ran is offered no second time"
    );
}

#[test]
fn a_relist_while_the_emptying_waits_still_asks_with_the_fresh_listing() {
    let a = item("a.txt", Some("docs"), Some(5));
    let mut state = in_trash(std::slice::from_ref(&a));
    let effects = state.trash_command("empty-trash").expect("a command");
    let first = the_load(&effects);
    // Before that listing comes, the window comes to the front and the trash is read again.
    let effects = state.window_focused();
    let second = the_load(&effects);
    assert_ne!(first.generation, second.generation);
    // The first listing is stale now; the second one asks, with what it found.
    let c = item("c.txt", Some("docs"), Some(7));
    list_trash(&mut state, &first, std::slice::from_ref(&a));
    assert_eq!(state.modal(), None);
    list_trash(&mut state, &second, &[a.clone(), c.clone()]);
    assert_eq!(
        confirm(&state).message,
        "휴지통의 항목 2개(12 바이트)를 모두 영구 삭제할까요?\n되돌릴 수 없습니다."
    );
    assert_eq!(
        confirm(&state).purpose,
        ConfirmPurpose::EmptyTrash(vec![a.clone(), c])
    );
    assert_eq!(state.dialog_cancelled(), []);
    // F5 meanwhile, too.
    let effects = state.trash_command("empty-trash").expect("a command");
    let first = the_load(&effects);
    let effects = press(&mut state, &special(Key::F5), NONE).expect("taken");
    let second = the_load(&effects);
    list_trash(&mut state, &first, std::slice::from_ref(&a));
    assert_eq!(state.modal(), None);
    list_trash(&mut state, &second, std::slice::from_ref(&a));
    assert!(confirm(&state).message.contains("항목 1개"));
}

#[test]
fn a_place_of_the_shell_lists_no_folders_in_the_address_bar() {
    let mut state = in_trash(&[item("folder", Some("docs"), None)]);
    // The `›` of its piece lists nothing (the trash's items are no folders to go into).
    assert_eq!(state.open_subfolders(LEFT, 0), []);
    assert!(state.subfolders().is_none());
    // Nor does the path field complete anything in it.
    for text in [
        "shell://trash/",
        "shell://trash/f",
        "휴지통",
        "shell://computer/",
    ] {
        let effects = state.path_edited(LEFT, text);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::ListNames(_))),
            "{text}: {effects:?}"
        );
    }
    let mut state = ready(vec![], vec![]);
    let request = go(&mut state, computer());
    state.list_computer(request.generation);
    assert_eq!(state.open_subfolders(LEFT, 0), []);
}

#[test]
fn the_selection_tools_go_by_the_names_the_trash_shows() {
    let mut state = in_trash(&[
        item("a.txt", Some("docs"), Some(1)),
        item("b.md", Some("docs"), Some(1)),
        item("README", Some("docs"), Some(1)),
    ]);
    // 패턴으로 선택…: `a*` is the item that was called a.txt, not its id.
    press(&mut state, "s", CTRL);
    let effects = state.pattern_edited("a*");
    let request = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::MatchNames(request) => Some(request.clone()),
            _ => None,
        })
        .expect("the names are matched");
    state.pattern_confirmed("a*");
    state.pattern_matched(request.run(&|| true).expect("not stopped"));
    let tab = state.panes()[LEFT].active_tab();
    let chosen: Vec<&std::ffi::OsStr> = tab
        .selection()
        .indices(tab.visible())
        .into_iter()
        .map(|row| tab.visible()[row].shown_name())
        .collect();
    assert_eq!(chosen, ["a.txt"]);
    // 같은 확장자 선택 on a row without one says the name it shows.
    choose(&mut state, "README");
    let effects = press(&mut state, "S", CTRL_SHIFT).expect("taken");
    assert_eq!(notices(&effects), [words::no_extension("README").as_str()]);
}

#[test]
fn no_refusal_shows_under_an_open_window() {
    let mut state = in_trash(&[item("a.txt", Some("docs"), Some(1))]);
    choose(&mut state, "a.txt");
    state.trash_command("delete-from-trash");
    assert!(matches!(state.modal(), Some(Modal::Confirm(_))));
    for effects in [
        state.new_file(),
        state.paste_from_menu(false),
        state.copy_paths_as(PathForm::of_key(Os::CURRENT)),
        state.folder_properties(),
        state.folder_terminal(),
        state.favorite_add_selected(),
    ] {
        assert_eq!(effects, []);
    }
}

#[test]
fn another_app_s_drag_over_the_sidebar_s_trash_is_told_why_it_is_refused() {
    let mut state = ready(vec![], vec![]);
    let [my_pc, bin] = shell_places();
    state.set_places(&[my_pc, drive("시스템 디스크", "root", 50, 100), bin]);
    let dropped = rf_platform::dnd::DroppedItems {
        paths: vec![
            folder("x")
                .join("a.txt")
                .expect("a name")
                .as_local()
                .expect("a local path")
                .to_path_buf(),
        ],
        ..rf_platform::dnd::DroppedItems::default()
    };
    let hover = state.outside_hover(
        Some(DropSpot::Tree { tree: 0, row: 2 }),
        &dropped,
        DragMods::default(),
        rf_platform::dnd::Effects::ALL,
    );
    assert_eq!(hover, Hover::Refused(crate::pick::Validity::ForeignToTrash));
    assert_eq!(
        words::drop_refused(crate::pick::Validity::ForeignToTrash),
        "여기에 놓을 수 없습니다: 다른 앱에서 끌어 온 항목은 그 앱에서 휴지통으로 보내세요"
    );
    // 내 PC says what it says of a list of the shell.
    let hover = state.outside_hover(
        Some(DropSpot::Tree { tree: 0, row: 0 }),
        &dropped,
        DragMods::default(),
        rf_platform::dnd::Effects::ALL,
    );
    assert_eq!(hover, Hover::Refused(crate::pick::Validity::ShellPlace));
}

#[test]
fn finding_the_same_drives_again_keeps_the_sidebar_as_it_was() {
    let mut state = ready(vec![], vec![]);
    let home = Place {
        label: "홈".to_owned(),
        path: folder("home"),
        kind: PlaceKind::Favorite(rf_platform::KnownFolder::Home),
        space: None,
    };
    let [my_pc, bin] = shell_places();
    let places = |available: u64| {
        vec![
            home.clone(),
            my_pc.clone(),
            drive("시스템 디스크", "root", available, 100),
            bin.clone(),
        ]
    };
    state.set_places(&places(50));
    // The home folder opens in the sidebar.
    let request = state.toggle_tree_row(0, 0);
    let request = the_load_children(&request);
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("docs")],
    });
    let open = state.tree().rows().len();
    assert_eq!(open, 5);
    // F5 in 내 PC finds the same places, with another free space: the tree stays open.
    state.set_places(&places(40));
    assert_eq!(state.tree().rows().len(), open);
    let space = state
        .places()
        .iter()
        .find(|place| place.kind == PlaceKind::Drive)
        .and_then(|place| place.space);
    assert_eq!(space.map(|space| space.available), Some(40));
    // Other drives start the tree again.
    let mut more = places(40);
    more.insert(3, drive("USB", "usb", 1, 2));
    state.set_places(&more);
    assert_eq!(state.tree().rows().len(), 5);
    assert!(!state.tree().rows()[0].expanded);
}

/// The one listing of a tree's node among `effects`.
fn the_load_children(effects: &[Effect]) -> LoadRequest {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::LoadChildren(request) => Some(request.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a listing of a node: {effects:?}"))
}
