//! Headless checks of the Slint screens (no window system needed).

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
// The input method's composition has no public event; Slint's own backends send it this way.
use i_slint_core::input::{InternalKeyEvent, KeyEventType};
use rf_core::{Entry, EntryKind, VPath};
use rf_platform::{DiskSpace, KnownFolder};
use rf_ui::loader::LoadEvent;
use rf_ui::pane::Pane;
use rf_ui::places::{Place, PlaceKind};
use rf_ui::selection::Selection;
use rf_ui::tree::Tree;
use rf_ui::ui_bridge::{self, FileRows, PaneModels, TreeRows};
use rf_ui::{
    ConfirmView, ConflictView, DialogKind, DropPlace, JobRowView, MainWindow, NameView, PaneView,
    PickTarget, PickView, Theme,
};
use rf_vfs::VisibilityFilter;
use slint::language::DragAction;
use slint::platform::{Key, WindowEvent};
use slint::{Color, ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc};

/// A shown main window at the default size, on the testing backend.
fn window() -> MainWindow {
    i_slint_backend_testing::init_no_event_loop();
    let window = MainWindow::new().expect("MainWindow builds");
    window.window().set_size(LogicalSize::new(1280.0, 800.0));
    window.show().expect("the window shows");
    window
}

fn entry(name: &str, kind: EntryKind, size: u64) -> Entry {
    let mut entry = Entry::new(name, kind);
    entry.size = size;
    entry
}

/// A pane at a temporary folder that has listed `entries`.
fn listed_pane(entries: Vec<Entry>) -> Pane {
    let folder = VPath::local(std::env::temp_dir().join("listed")).expect("a local path");
    let (mut pane, request) = Pane::new(folder, VisibilityFilter::default());
    pane.handle(LoadEvent::Listed {
        generation: request.generation,
        entries,
    });
    pane
}

/// The root of the system's drive, which `VPath::local` takes on every system (`C:\` on
/// Windows, where a bare `\` names no drive, and `/` elsewhere), and the sidebar name
/// `places::drives` gives it.
fn system_drive() -> (VPath, String) {
    if cfg!(windows) {
        let root = VPath::local(r"C:\").expect("the root is local");
        (root, rf_ui::words::drive_letter('C'))
    } else {
        let root = VPath::local("/").expect("the root is local");
        (root, rf_ui::words::system_disk().to_owned())
    }
}

/// The elements whose accessible label is `label` and whose role is `role` (a text's label
/// is its text, so a button's label is often on its text too).
fn labelled(window: &MainWindow, label: &str, role: AccessibleRole) -> Vec<ElementHandle> {
    ElementHandle::find_by_accessible_label(window, label)
        .filter(|element| element.accessible_role() == Some(role))
        .collect()
}

#[test]
fn main_window_builds_with_theme() {
    i_slint_backend_testing::init_no_event_loop();
    let window = MainWindow::new().expect("MainWindow builds");
    let theme = window.global::<Theme<'_>>();

    assert!(!theme.get_dark());
    assert_eq!(theme.get_bg(), Color::from_rgb_u8(0xF6, 0xF7, 0xF9));
    assert_eq!(theme.get_accent(), Color::from_rgb_u8(0x2F, 0x6F, 0xEB));
    assert_eq!(theme.get_selection(), Color::from_rgb_u8(0xE6, 0xEE, 0xFD));

    theme.set_dark(true);
    assert_eq!(theme.get_bg(), Color::from_rgb_u8(0x15, 0x17, 0x1A));
    assert_eq!(theme.get_accent(), Color::from_rgb_u8(0x78, 0xA1, 0xF2));
    assert_eq!(theme.get_selection(), Color::from_rgb_u8(0x31, 0x3C, 0x51));
}

#[test]
fn pane_view_shows_rows_from_the_model() {
    let window = window();
    let mut pane = listed_pane(vec![
        entry("docs", EntryKind::Dir, 0),
        entry("notes.txt", EntryKind::File, 3100),
        entry("설계안.pdf", EntryKind::File, 1_258_291),
    ]);
    {
        let tab = pane.active_tab_mut();
        let (selection, rows) = tab.selection_mut();
        selection.click(1, rows);
        selection.toggle(2, rows);
    }
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));

    let view = window.get_left_pane();
    assert!(view.active && !view.loading);
    assert_eq!(view.error, "");
    assert_eq!(view.cursor, 2);
    assert_eq!(view.status, "항목 3개 · 선택 2개 (1.2 MB)");
    assert_eq!(view.rows.row_count(), 3);
    let row = view.rows.row_data(2).expect("a third row");
    assert_eq!(
        (row.name.as_str(), row.size.as_str()),
        ("설계안.pdf", "1.2 MB")
    );
    assert!(row.selected);
    let tabs: Vec<_> = view.tabs.iter().collect();
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].title, "listed");
    assert_eq!(
        view.crumbs.iter().last().map(|c| c.label),
        Some("listed".into())
    );

    // The list draws the rows of the model, selected as the model says.
    let rows = labelled(&window, "notes.txt", AccessibleRole::ListItem);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].accessible_item_selected(), Some(true));
    assert_eq!(
        labelled(&window, "docs", AccessibleRole::ListItem)[0].accessible_item_selected(),
        Some(false)
    );

    // A change of the model reaches the list without a new view.
    {
        let tab = pane.active_tab_mut();
        let (selection, _) = tab.selection_mut();
        selection.clear();
    }
    window.set_left_pane(models.view(&pane, true));
    assert_eq!(
        labelled(&window, "notes.txt", AccessibleRole::ListItem)[0].accessible_item_selected(),
        Some(false)
    );
}

#[test]
fn key_callback_reaches_rust() {
    let window = window();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&seen);
    window.on_key(move |text, ctrl, shift, alt| {
        record
            .borrow_mut()
            .push((text.to_string(), ctrl, shift, alt));
        true
    });

    assert!(window.invoke_key("a".into(), true, false, false));
    assert_eq!(
        seen.borrow().as_slice(),
        [("a".to_owned(), true, false, false)]
    );

    // A key pressed in the window reaches the callback through the window's key scope.
    let down = char::from(Key::DownArrow).to_string();
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: down.clone().into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: down.clone().into(),
    });
    assert_eq!(seen.borrow().last(), Some(&(down, false, false, false)));
}

/// Presses and releases `keys` in order, then releases them in reverse: `[Control, "t"]` is
/// Ctrl+T.
fn chord(window: &MainWindow, keys: &[&str]) {
    for key in keys {
        window.window().dispatch_event(WindowEvent::KeyPressed {
            text: (*key).into(),
        });
    }
    for key in keys.iter().rev() {
        window.window().dispatch_event(WindowEvent::KeyReleased {
            text: (*key).into(),
        });
    }
}

#[test]
fn keys_go_to_the_path_field_while_it_is_open() {
    let window = window();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&seen);
    window.on_key(move |text, ctrl, _, _| {
        record.borrow_mut().push((text.to_string(), ctrl));
        true
    });
    let entered = Rc::new(RefCell::new(None));
    let record = Rc::clone(&entered);
    window.on_path_entered(move |side, text| {
        *record.borrow_mut() = Some((side, text.to_string()));
    });
    let control = char::from(Key::Control).to_string();
    let down = char::from(Key::DownArrow).to_string();
    let enter = char::from(Key::Return).to_string();

    // Without the field, Ctrl+T and Down are shortcuts.
    chord(&window, &[&control, "t"]);
    chord(&window, &[&down]);
    let shortcuts: Vec<_> = seen
        .borrow_mut()
        .drain(..)
        .filter(|(text, _)| *text != control)
        .collect();
    assert_eq!(shortcuts, [("t".to_owned(), true), (down.clone(), false)]);

    let mut view = window.get_right_pane();
    view.path_text = "/start".into();
    window.set_right_pane(view);
    window.invoke_edit_path(1);
    // The field takes the text; the keys it does not use are not shortcuts while it is open.
    chord(&window, &["x"]);
    chord(&window, &[&control, "t"]);
    chord(&window, &[&down]);
    assert_eq!(seen.borrow().as_slice(), []);
    chord(&window, &[&enter]);
    // The field had the whole path selected, so the typed text replaced it.
    assert_eq!(*entered.borrow(), Some((1, "x".to_owned())));
    assert_eq!(seen.borrow().as_slice(), []);
}

#[test]
fn rows_that_come_and_go_reach_the_list() {
    let window = window();
    let rows = Rc::new(FileRows::default());
    let names = |list: &[&str]| -> Vec<Entry> {
        list.iter()
            .map(|name| entry(name, EntryKind::File, 1))
            .collect()
    };
    let none = Selection::default();
    rows.sync(&names(&["a.txt", "b.txt", "c.txt"]), &none);
    window.set_left_pane(PaneView {
        rows: ModelRc::from(Rc::clone(&rows)),
        ..PaneView::default()
    });
    // The rows the left list draws, in their order.
    let drawn = || -> Vec<String> {
        let mut items: Vec<(usize, String)> = window
            .root_element()
            .query_descendants()
            .match_accessible_role(AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .filter_map(|row| Some((row.accessible_item_index()?, row.accessible_label()?.into())))
            .collect();
        items.sort();
        items.into_iter().map(|(_, label)| label).collect()
    };
    assert_eq!(drawn(), ["a.txt", "b.txt", "c.txt"]);

    // Another app adds one file and removes another: the model reports the two rows, and the
    // list draws the new set without a new view.
    rows.sync(&names(&["a.txt", "ab.txt", "c.txt"]), &none);
    assert_eq!(drawn(), ["a.txt", "ab.txt", "c.txt"]);
}

#[test]
fn sort_header_invokes_callback() {
    let window = window();
    let sorted = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&sorted);
    window.on_sort(move |side, column| record.borrow_mut().push((side, column)));

    // Both panes have the heads; the left one comes first.
    let size_heads = labelled(&window, "크기", AccessibleRole::Button);
    assert_eq!(size_heads.len(), 2);
    size_heads[0].invoke_accessible_default_action();
    size_heads[1].invoke_accessible_default_action();
    labelled(&window, "이름", AccessibleRole::Button)[0].invoke_accessible_default_action();
    assert_eq!(sorted.borrow().as_slice(), [(0, 1), (1, 1), (0, 0)]);
}

#[test]
fn the_tree_shows_places_and_folders() {
    const GB: u64 = 1 << 30;
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&calls);
    window.on_tree_clicked(move |i| record.borrow_mut().push(("clicked", i)));
    let record = Rc::clone(&calls);
    window.on_tree_toggled(move |i| record.borrow_mut().push(("toggled", i)));

    let home = VPath::local(std::env::temp_dir().join("home")).expect("a local path");
    let (disk, disk_label) = system_drive();
    let places = [
        Place {
            label: "홈".to_owned(),
            path: home.clone(),
            kind: PlaceKind::Favorite(KnownFolder::Home),
            space: None,
        },
        Place {
            label: disk_label.clone(),
            path: disk,
            kind: PlaceKind::Drive,
            space: Some(DiskSpace {
                total: 512 * GB,
                free: 130 * GB,
                available: 128 * GB,
            }),
        },
    ];
    let mut tree = Tree::new(VisibilityFilter::default());
    tree.set_roots(
        places
            .iter()
            .map(|place| (place.label.clone(), place.path.clone())),
    );
    let rows = Rc::new(TreeRows::default());
    rows.sync(&tree, &places);
    window.set_tree(ModelRc::from(Rc::clone(&rows)));

    assert_eq!(labelled(&window, "즐겨찾기", AccessibleRole::Text).len(), 1);
    assert_eq!(labelled(&window, "이 PC", AccessibleRole::Text).len(), 1);
    let drive = labelled(&window, &disk_label, AccessibleRole::ListItem);
    assert_eq!(
        drive[0].accessible_description().as_deref(),
        Some("128 GB 남음 / 512 GB")
    );
    assert_eq!(drive[0].accessible_expandable(), Some(true));
    assert_eq!(drive[0].accessible_expanded(), Some(false));
    // A click shows the row's folder; the chevron (the expand action) opens the row.
    drive[0].invoke_accessible_default_action();
    labelled(&window, "홈", AccessibleRole::ListItem)[0].invoke_accessible_expand_action();
    assert_eq!(calls.borrow().as_slice(), [("clicked", 1), ("toggled", 0)]);

    // Home opens and its folders come into the list, the pane's folder selected, without a
    // new model.
    let request = tree.toggle(0).expect("home is listed");
    tree.apply_children(
        request.generation,
        vec![
            Entry::new("a", EntryKind::Dir),
            Entry::new("b", EntryKind::Dir),
        ],
    );
    tree.reveal(&home.join("b").expect("a plain name"));
    rows.sync(&tree, &places);
    let mut drawn: Vec<(usize, String, bool)> = window
        .root_element()
        .query_descendants()
        .match_accessible_role(AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .filter_map(|row| {
            Some((
                row.accessible_item_index()?,
                row.accessible_label()?.into(),
                row.accessible_item_selected()?,
            ))
        })
        .collect();
    drawn.sort();
    assert_eq!(
        drawn,
        [
            (0, "홈".to_owned(), false),
            (1, "a".to_owned(), false),
            (2, "b".to_owned(), true),
            (3, disk_label, false),
        ]
    );
    let home_row = labelled(&window, "홈", AccessibleRole::ListItem);
    assert_eq!(home_row[0].accessible_expanded(), Some(true));
}

#[test]
fn the_job_panel_shows_its_rows_and_buttons() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::new()));
    for (name, record) in [("pause", Rc::clone(&calls)), ("cancel", Rc::clone(&calls))] {
        let record = Rc::clone(&record);
        if name == "pause" {
            window.on_job_pause(move |i| record.borrow_mut().push(("pause", i)));
        } else {
            window.on_job_cancel(move |i| record.borrow_mut().push(("cancel", i)));
        }
    }
    assert!(labelled(&window, "일시정지", AccessibleRole::Button).is_empty());
    let rows = Rc::new(slint::VecModel::from(vec![JobRowView {
        title: "복사 중".into(),
        detail: "assets → /home/me/백업".into(),
        progress: 0.25,
        progress_text: "3 / 12 항목 · 48 MB/s · 남은 시간 약 20초 · 대기 중인 작업 1개".into(),
        paused: false,
        waiting: false,
    }]));
    window.set_job_rows(ModelRc::from(Rc::clone(&rows)));
    assert_eq!(labelled(&window, "복사 중", AccessibleRole::Text).len(), 1);
    assert_eq!(
        labelled(
            &window,
            "3 / 12 항목 · 48 MB/s · 남은 시간 약 20초 · 대기 중인 작업 1개",
            AccessibleRole::Text
        )
        .len(),
        1
    );
    labelled(&window, "일시정지", AccessibleRole::Button)[0].invoke_accessible_default_action();
    labelled(&window, "취소", AccessibleRole::Button)[0].invoke_accessible_default_action();
    assert_eq!(calls.borrow().as_slice(), [("pause", 0), ("cancel", 0)]);
    // A paused job offers to go on.
    rows.set_row_data(
        0,
        JobRowView {
            paused: true,
            title: "일시정지됨".into(),
            ..rows.row_data(0).expect("a row")
        },
    );
    assert_eq!(labelled(&window, "계속", AccessibleRole::Button).len(), 1);
}

#[test]
fn a_toast_button_answers_with_its_token() {
    let window = window();
    let clicked = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&clicked);
    window.on_notice_action(move |token| record.borrow_mut().push(("action", token)));
    let record = Rc::clone(&clicked);
    window.on_notice_closed(move |token| record.borrow_mut().push(("closed", token)));
    window.invoke_show_toast("항목 3개를 이동했습니다".into(), "되돌리기".into(), 7);
    assert!(!labelled(&window, "항목 3개를 이동했습니다", AccessibleRole::Text).is_empty());
    labelled(&window, "되돌리기", AccessibleRole::Button)
        .into_iter()
        .find(|button| button.accessible_enabled() != Some(false))
        .expect("the toast's button")
        .invoke_accessible_default_action();
    assert_eq!(clicked.borrow().as_slice(), [("action", 7)]);
    // A button that would no longer do what it says is hidden.
    window.invoke_show_toast("항목 1개를 이동했습니다".into(), "되돌리기".into(), 9);
    assert_eq!(
        labelled(&window, "되돌리기", AccessibleRole::Button).len(),
        2
    );
    window.set_notice_action_live(false);
    assert_eq!(
        labelled(&window, "되돌리기", AccessibleRole::Button).len(),
        1
    );
    window.set_notice_action_live(true);
    // A plain notice has no button; replacing a toast tells the old one is gone.
    window.invoke_show_toast("다른 알림".into(), "자세히".into(), 8);
    window.invoke_show_notice("알림".into());
    assert_eq!(
        clicked.borrow().as_slice(),
        [("action", 7), ("closed", 9), ("closed", 8)]
    );
    assert!(labelled(&window, "자세히", AccessibleRole::Button).is_empty());
}

#[test]
fn a_dialog_takes_the_keys_until_it_closes() {
    let window = window();
    let keys = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&keys);
    window.on_key(move |text, _, _, _| {
        record.borrow_mut().push(text.to_string());
        true
    });
    let answers = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&answers);
    window.on_confirm_accepted(move || record.borrow_mut().push("accepted"));
    let record = Rc::clone(&answers);
    window.on_dialog_cancelled(move || record.borrow_mut().push("cancelled"));

    window.set_confirm_view(ConfirmView {
        title: "영구 삭제".into(),
        message: "휴지통을 쓸 수 없는 위치입니다.\n항목 1개를 영구 삭제할까요? 되돌릴 수 없습니다."
            .into(),
        detail: "‘a.txt’".into(),
        ok_text: "영구 삭제".into(),
        danger: true,
    });
    window.set_dialog(DialogKind::Confirm);
    let message =
        "휴지통을 쓸 수 없는 위치입니다.\n항목 1개를 영구 삭제할까요? 되돌릴 수 없습니다.";
    assert_eq!(labelled(&window, message, AccessibleRole::Text).len(), 1);
    // The panes get no key while it shows, and Enter does not delete.
    let down = char::from(Key::DownArrow).to_string();
    let enter = char::from(Key::Return).to_string();
    chord(&window, &[&down]);
    chord(&window, &[&enter]);
    assert_eq!(keys.borrow().as_slice(), Vec::<String>::new());
    assert_eq!(answers.borrow().as_slice(), Vec::<&str>::new());
    // Esc is No.
    chord(&window, &[&char::from(Key::Escape).to_string()]);
    assert_eq!(answers.borrow().as_slice(), ["cancelled"]);
    // Only the button deletes.
    let delete: Vec<ElementHandle> = labelled(&window, "영구 삭제", AccessibleRole::Button);
    delete[0].invoke_accessible_default_action();
    assert_eq!(answers.borrow().as_slice(), ["cancelled", "accepted"]);

    // Closed, the panes have the keys again.
    window.set_dialog(DialogKind::None);
    chord(&window, &[&down]);
    assert_eq!(keys.borrow().as_slice(), [down]);
}

#[test]
fn the_failures_window_shows_whole_paths_wrapped() {
    let window = window();
    // A Windows path has no place to break a line at its `\`; a short line has one line.
    let long = format!(
        "C:\\{}\\a.txt: 권한이 없습니다",
        "VeryLongFolderNameWithoutBreaks".repeat(4)
    );
    let short = "a.txt: 권한이 없습니다".to_owned();
    let failures = rf_ui::dialogs::Failures {
        title: "처리하지 못한 항목".to_owned(),
        lines: vec![long.clone(), short.clone()],
    };
    window.set_message_view(ui_bridge::message_view(&failures));
    window.set_dialog(DialogKind::Message);

    // Each line shows once: the copy the window measures is out of sight.
    let long_line = labelled(&window, &long, AccessibleRole::Text);
    let short_line = labelled(&window, &short, AccessibleRole::Text);
    assert_eq!((long_line.len(), short_line.len()), (1, 1));
    let (long_line, short_line) = (&long_line[0], &short_line[0]);
    // The long one wraps within the list, onto more lines than the short one.
    let list = window
        .root_element()
        .query_descendants()
        .match_accessible_role(AccessibleRole::List)
        .find_all()
        .into_iter()
        .find(|list| {
            let (at, size) = (list.absolute_position(), list.size());
            let line = long_line.absolute_position();
            (at.x..at.x + size.width).contains(&line.x)
                && (at.y..at.y + size.height).contains(&line.y)
        })
        .expect("the list of the window");
    let (list_at, list_size) = (list.absolute_position(), list.size());
    let right = long_line.absolute_position().x + long_line.size().width;
    assert!(
        right <= list_at.x + list_size.width,
        "{right} past the list"
    );
    assert!(long_line.size().height > short_line.size().height);
    // The list is as tall as both lines: nothing to scroll to.
    let bottom = short_line.absolute_position().y + short_line.size().height;
    assert!(
        bottom <= list_at.y + list_size.height,
        "{bottom} below the list"
    );
}

/// The list among the window's lists that holds `line`.
fn list_around(window: &MainWindow, line: &ElementHandle) -> ElementHandle {
    window
        .root_element()
        .query_descendants()
        .match_accessible_role(AccessibleRole::List)
        .find_all()
        .into_iter()
        .find(|list| {
            let (at, size) = (list.absolute_position(), list.size());
            let line = line.absolute_position();
            (at.x..at.x + size.width).contains(&line.x)
                && (at.y..at.y + size.height).contains(&line.y)
        })
        .expect("the list of the window")
}

#[test]
fn the_preflight_window_fits_its_list_to_wrapped_lines() {
    let window = window();
    // A name with no place to break wraps onto several lines; a short one takes two (its
    // problem, and what happens to it).
    let long_name = format!("{}.txt", "VeryLongFileNameWithoutBreaks".repeat(8));
    let issue = |name: &str| rf_jobs::Issue {
        target: VPath::local(std::env::temp_dir().join(name)).expect("a local path"),
        kind: rf_jobs::IssueKind::NameTooLong { len: 300, max: 255 },
    };
    let question = rf_ui::dialogs::PreflightQuestion {
        id: 1,
        job: rf_jobs::JobId(1),
        issues: vec![issue(&long_name), issue("a.txt")],
    };
    let lines = question.lines();
    window.set_preflight_view(ui_bridge::preflight_view(&question));
    window.set_dialog(DialogKind::Preflight);

    // Each line shows once: the copy the window measures is out of sight.
    let long_line = labelled(&window, &lines[0], AccessibleRole::Text);
    let short_line = labelled(&window, &lines[1], AccessibleRole::Text);
    assert_eq!((long_line.len(), short_line.len()), (1, 1));
    let (long_line, short_line) = (&long_line[0], &short_line[0]);
    let list = list_around(&window, long_line);
    let (list_at, list_size) = (list.absolute_position(), list.size());
    let right = long_line.absolute_position().x + long_line.size().width;
    assert!(
        right <= list_at.x + list_size.width,
        "{right} past the list"
    );
    assert!(long_line.size().height > short_line.size().height);
    // The list is as tall as both lines: nothing to scroll to.
    let bottom = short_line.absolute_position().y + short_line.size().height;
    assert!(
        bottom <= list_at.y + list_size.height,
        "{bottom} below the list"
    );
}

#[test]
fn the_name_dialog_replaces_the_stem() {
    let window = window();
    let edits = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&edits);
    window.on_name_edited(move |text| record.borrow_mut().push(("edited", text.to_string())));
    let record = Rc::clone(&edits);
    window.on_name_accepted(move |text| record.borrow_mut().push(("accepted", text.to_string())));
    window.set_name_view(NameView {
        title: "이름 바꾸기".into(),
        initial: "설계안.pdf".into(),
        select_end: i32::try_from("설계안".len()).expect("short"),
        warning: "".into(),
        ok_enabled: false,
    });
    window.set_dialog(DialogKind::Name);
    // Typing replaces the selected stem and keeps the extension; Enter hands the name over.
    chord(&window, &["x"]);
    chord(&window, &[&char::from(Key::Return).to_string()]);
    assert_eq!(
        edits.borrow().as_slice(),
        [
            ("edited", "x.pdf".to_owned()),
            ("accepted", "x.pdf".to_owned())
        ]
    );
    // OK is off until Rust says the name can be used.
    let ok = labelled(&window, "확인", AccessibleRole::Button);
    assert_eq!(ok[0].accessible_enabled(), Some(false));
}

#[test]
fn a_name_being_composed_holds_the_name_dialog() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&calls);
    window.on_name_edited(move |text| record.borrow_mut().push(format!("edited {text}")));
    let record = Rc::clone(&calls);
    window.on_name_accepted(move |text| record.borrow_mut().push(format!("accepted {text}")));
    window.set_name_view(NameView {
        title: "새 폴더".into(),
        initial: "보고서".into(),
        select_end: 0,
        warning: "".into(),
        ok_enabled: true,
    });
    window.set_dialog(DialogKind::Name);
    slint::platform::update_timers_and_animations();
    let field = || labelled(&window, "새 폴더", AccessibleRole::TextInput).remove(0);
    let ok = || labelled(&window, "확인", AccessibleRole::Button).remove(0);
    let enter = char::from(Key::Return).to_string();
    // Typed at the end of the name, as the cursor is after a click there.
    field().mock_single_click(slint::platform::PointerEventButton::Left);
    chord(&window, &[&char::from(Key::End).to_string()]);
    assert_eq!(ok().accessible_enabled(), Some(true));

    // While a syllable is composed the name is still the one before it, and Rust has heard of
    // no edit: neither the button nor Enter makes a folder with the syllable missing.
    compose(&window, "ㅅ");
    slint::platform::update_timers_and_animations();
    assert_eq!(field().accessible_value().as_deref(), Some("보고서"));
    assert_eq!(ok().accessible_enabled(), Some(false));
    ok().invoke_accessible_default_action();
    chord(&window, &[&enter]);
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());

    // The input method cancels the composition: the name is as it was, and can be used again.
    compose(&window, "");
    slint::platform::update_timers_and_animations();
    assert_eq!(field().accessible_value().as_deref(), Some("보고서"));
    assert_eq!(ok().accessible_enabled(), Some(true));
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());

    // A committed syllable is part of the name, which Enter hands over.
    compose(&window, "ㅅ");
    commit_composition(&window, "서");
    slint::platform::update_timers_and_animations();
    assert_eq!(ok().accessible_enabled(), Some(true));
    chord(&window, &[&enter]);
    assert_eq!(
        calls.borrow().as_slice(),
        ["edited 보고서서", "accepted 보고서서"]
    );
}

fn conflict_view() -> ConflictView {
    ConflictView {
        serial: 1,
        title: "같은 이름의 파일이 이미 있습니다".into(),
        description: "‘설계안.pdf’가 /home/me/백업 에 이미 있습니다.".into(),
        incoming_label: "옮겨 올 파일".into(),
        incoming_name: "설계안.pdf".into(),
        incoming_place: "/home/me/프로젝트".into(),
        incoming_size: "1.2 MB (1,258,291 바이트)".into(),
        incoming_time: "2026-10-25 16:20".into(),
        incoming_newer: true,
        existing_label: "이미 있는 파일".into(),
        existing_name: "설계안.pdf".into(),
        existing_place: "/home/me/백업".into(),
        existing_size: "980 KB (1,003,520 바이트)".into(),
        existing_time: "2026-08-02 11:04".into(),
        can_overwrite: true,
        overwrite_hint: "있던 파일을 옮겨 올 파일로 바꿉니다".into(),
        rename_hint: "‘설계안 (2).pdf’처럼 번호를 붙여 저장합니다".into(),
        warning_once: "있던 파일의 내용은 되살릴 수 없습니다.".into(),
        warning_all: "남은 충돌도 모두 덮어씁니다. 있던 파일의 내용은 되살릴 수 없습니다.".into(),
        ..ConflictView::default()
    }
}

#[test]
fn the_conflict_dialog_keeps_both_unless_overwrite_is_clicked() {
    let window = window();
    let answers = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&answers);
    window.on_conflict_answered(move |choice, all| record.borrow_mut().push((choice, all)));
    window.set_conflict_view(conflict_view());
    window.set_dialog(DialogKind::Conflict);
    assert_eq!(labelled(&window, "더 최근", AccessibleRole::Text).len(), 1);
    // Keeping both is chosen at first; Enter takes it.
    let rename = labelled(&window, "이름 바꿔 둘 다 두기", AccessibleRole::RadioButton);
    assert_eq!(rename[0].accessible_checked(), Some(true));
    chord(&window, &[&char::from(Key::Return).to_string()]);
    assert_eq!(answers.borrow().as_slice(), [(2, false)]);

    // Overwrite says what it does, and only its button confirms it.
    labelled(&window, "덮어쓰기", AccessibleRole::RadioButton)[0]
        .invoke_accessible_default_action();
    assert_eq!(
        labelled(
            &window,
            "있던 파일의 내용은 되살릴 수 없습니다.",
            AccessibleRole::Text
        )
        .len(),
        1
    );
    chord(&window, &[&char::from(Key::Return).to_string()]);
    assert_eq!(answers.borrow().len(), 1);
    labelled(
        &window,
        "남은 충돌에도 같은 선택 적용",
        AccessibleRole::Checkbox,
    )[0]
    .invoke_accessible_default_action();
    labelled(&window, "덮어쓰기", AccessibleRole::Button)[0].invoke_accessible_default_action();
    assert_eq!(answers.borrow().as_slice(), [(2, false), (0, true)]);

    // A new question starts again from keeping both; a file and a folder offer no overwrite.
    window.set_conflict_view(ConflictView {
        serial: 2,
        can_overwrite: false,
        existing_dir: true,
        ..conflict_view()
    });
    let overwrite = labelled(&window, "덮어쓰기", AccessibleRole::RadioButton);
    assert_eq!(overwrite[0].accessible_enabled(), Some(false));
    assert_eq!(
        labelled(&window, "이름 바꿔 둘 다 두기", AccessibleRole::RadioButton)[0]
            .accessible_checked(),
        Some(true)
    );
}

/// A picker of a move whose target, the other pane's folder, is chosen; the items' own folder
/// is offered too, and cannot be chosen.
fn pick_view() -> PickView {
    let targets = vec![
        PickTarget {
            head: "반대편 패널".into(),
            label: "백업".into(),
            note: "/home/me/작업/백업".into(),
            enabled: true,
            selected: true,
        },
        PickTarget {
            head: "열린 탭".into(),
            label: "원본".into(),
            note: "원래 있던 폴더라 고를 수 없음".into(),
            enabled: false,
            selected: false,
        },
    ];
    let chips: Vec<slint::SharedString> = vec!["설계안.pdf".into(), "assets".into()];
    PickView {
        title: "어디로 이동할까요?".into(),
        summary: "항목 2개 · 파일 1.2 MB와 폴더 1개".into(),
        chips: ModelRc::new(slint::VecModel::from(chips)),
        is_move: true,
        target_path: "/home/me/작업/백업".into(),
        can_make_folder: true,
        targets: ModelRc::new(slint::VecModel::from(targets)),
        policy_note: "같은 이름이 있으면 그때 묻습니다".into(),
        space: "남은 공간 212 GB".into(),
        confirm_text: "여기로 이동".into(),
        can_confirm: true,
        ..PickView::default()
    }
}

#[test]
fn the_picker_takes_the_keys_until_it_closes() {
    let window = window();
    let keys = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&keys);
    window.on_key(move |text, _, _, _| {
        record.borrow_mut().push(text.to_string());
        true
    });
    let calls = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&calls);
    window.on_pick_step(move |delta| record.borrow_mut().push(format!("step {delta}")));
    let record = Rc::clone(&calls);
    window.on_pick_enter(move || record.borrow_mut().push("enter".to_owned()));
    let record = Rc::clone(&calls);
    window.on_pick_confirmed(move || record.borrow_mut().push("confirmed".to_owned()));
    let record = Rc::clone(&calls);
    window.on_pick_filter_edited(move |text| record.borrow_mut().push(format!("filter {text}")));
    let record = Rc::clone(&calls);
    window.on_pick_target_clicked(move |i| record.borrow_mut().push(format!("target {i}")));
    let record = Rc::clone(&calls);
    window.on_pick_policy(move |i| record.borrow_mut().push(format!("policy {i}")));
    let record = Rc::clone(&calls);
    window.on_dialog_cancelled(move || record.borrow_mut().push("cancelled".to_owned()));

    window.set_pick_view(pick_view());
    window.set_dialog(DialogKind::Pick);
    assert_eq!(
        labelled(&window, "어디로 이동할까요?", AccessibleRole::Text).len(),
        1
    );
    // Letters go to the filter (PICK-04); the arrows and Enter go to Rust, the panes get none.
    let down = char::from(Key::DownArrow).to_string();
    let up = char::from(Key::UpArrow).to_string();
    let enter = char::from(Key::Return).to_string();
    chord(&window, &["백"]);
    chord(&window, &[&down]);
    chord(&window, &[&up]);
    chord(&window, &[&enter]);
    chord(&window, &[&char::from(Key::F5).to_string()]);
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["filter 백", "step 1", "step -1", "enter"]
    );
    assert_eq!(keys.borrow().as_slice(), Vec::<String>::new());
    // A click between its parts leaves the keys with the filter: the letters still go there.
    let position = LogicalPosition::new(640.0, 184.0);
    let button = slint::platform::PointerEventButton::Left;
    let shown = window.window();
    shown.dispatch_event(WindowEvent::PointerPressed { position, button });
    shown.dispatch_event(WindowEvent::PointerReleased { position, button });
    chord(&window, &["업"]);
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["filter 백업"]
    );

    // The target the items' own folder is shows why it cannot be chosen, and takes no click.
    let items: Vec<ElementHandle> = labelled(&window, "원본", AccessibleRole::ListItem);
    assert_eq!(items[0].accessible_enabled(), Some(false));
    assert_eq!(
        items[0].accessible_description().as_deref(),
        Some("원래 있던 폴더라 고를 수 없음")
    );
    items[0].invoke_accessible_default_action();
    labelled(&window, "백업", AccessibleRole::ListItem)[0].invoke_accessible_default_action();
    labelled(&window, "덮어쓰기", AccessibleRole::RadioButton)[0]
        .invoke_accessible_default_action();
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["target 0", "policy 1"]
    );

    // The button confirms while the target can be chosen; Esc cancels.
    let confirm = labelled(&window, "여기로 이동", AccessibleRole::Button);
    confirm[0].invoke_accessible_default_action();
    window.set_pick_view(PickView {
        can_confirm: false,
        problem: "옮기는 항목 안이라 고를 수 없음".into(),
        ..pick_view()
    });
    let confirm = labelled(&window, "여기로 이동", AccessibleRole::Button);
    assert_eq!(confirm[0].accessible_enabled(), Some(false));
    assert_eq!(
        labelled(
            &window,
            "옮기는 항목 안이라 고를 수 없음",
            AccessibleRole::Text
        )
        .len(),
        1
    );
    chord(&window, &[&char::from(Key::Escape).to_string()]);
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["confirmed", "cancelled"]
    );

    // Closed, the panes have the keys again.
    window.set_dialog(DialogKind::None);
    chord(&window, &[&down]);
    assert_eq!(keys.borrow().as_slice(), [down]);
}

#[test]
fn the_picker_stays_under_its_name_window() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&calls);
    window.on_pick_step(move |delta| record.borrow_mut().push(format!("step {delta}")));
    let record = Rc::clone(&calls);
    window.on_pick_filter_edited(move |text| record.borrow_mut().push(format!("filter {text}")));
    window.set_pick_view(pick_view());
    window.set_dialog(DialogKind::Pick);
    let down = char::from(Key::DownArrow).to_string();

    // A name window over it (its 새 폴더) has the keys; the picker stays in view under it.
    window.set_pick_behind(true);
    window.set_name_view(NameView {
        title: "새 폴더".into(),
        initial: "새 폴더".into(),
        select_end: i32::try_from("새 폴더".len()).expect("short"),
        warning: "".into(),
        ok_enabled: true,
    });
    window.set_dialog(DialogKind::Name);
    chord(&window, &[&down]);
    chord(&window, &["x"]);
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());
    assert_eq!(
        labelled(&window, "어디로 이동할까요?", AccessibleRole::Text).len(),
        1
    );
    // Back on top, it has them again.
    window.set_dialog(DialogKind::Pick);
    window.set_pick_behind(false);
    chord(&window, &[&down]);
    assert_eq!(calls.borrow().as_slice(), ["step 1"]);
}

#[test]
fn the_theme_follows_the_view_menu() {
    let window = window();
    let theme = window.global::<Theme<'_>>();
    let chosen = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&chosen);
    window.on_theme_chosen(move |choice| record.borrow_mut().push(choice));
    let dark_bg = Color::from_rgb_u8(0x15, 0x17, 0x1A);
    let light_bg = Color::from_rgb_u8(0xF6, 0xF7, 0xF9);

    // The testing backend's system asks for no dark colors: 시스템 설정 따르기 is light.
    slint::platform::update_timers_and_animations();
    assert!(!theme.get_dark());
    for (choice, dark) in [(2, true), (0, false), (1, false), (2, true)] {
        window.set_theme_choice(choice);
        slint::platform::update_timers_and_animations();
        assert_eq!(theme.get_dark(), dark, "theme {choice}");
        assert_eq!(theme.get_bg(), if dark { dark_bg } else { light_bg });
    }

    // The 보기 button opens the menu whose items choose a theme.
    let button = labelled(&window, "보기", AccessibleRole::Button);
    assert_eq!(button.len(), 1);
    button[0].invoke_accessible_default_action();
    let dark = ElementHandle::find_by_accessible_label(&window, "어둡게")
        .next()
        .expect("the menu shows 어둡게");
    // The menu is a popup window of its own: the element clicks there.
    dark.mock_single_click(slint::platform::PointerEventButton::Left);
    assert_eq!(chosen.borrow().as_slice(), [2]);

    // The marks of the menu: the theme chosen, and 숨김 파일 보기 as `show-hidden` says.
    let commands = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&commands);
    window.on_command(move |name| record.borrow_mut().push(name.to_string()));
    let open_menu = || {
        button[0].invoke_accessible_default_action();
        ElementHandle::find_by_accessible_label(&window, "✓").count()
    };
    let click = |label: &str| {
        ElementHandle::find_by_accessible_label(&window, label)
            .next()
            .expect("an item of the menu")
            .mock_single_click(slint::platform::PointerEventButton::Left);
    };
    assert_eq!(open_menu(), 1);
    // Choosing the theme that is chosen keeps its mark.
    click("어둡게");
    assert_eq!(chosen.borrow().as_slice(), [2, 2]);
    assert_eq!(open_menu(), 1);
    click("어둡게");
    window.set_show_hidden(true);
    assert_eq!(open_menu(), 2);
    click("숨김 파일 보기");
    assert_eq!(commands.borrow().as_slice(), ["toggle-hidden"]);
}

#[test]
fn a_path_typed_and_not_entered_holds_the_button() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&calls);
    window.on_pick_confirmed(move || record.borrow_mut().push("confirmed".to_owned()));
    // Rust, as the controller does: a path typed and not entered turns the button off.
    let record = Rc::clone(&calls);
    let weak = window.as_weak();
    window.on_pick_path_edited(move |text| {
        record.borrow_mut().push(format!("edited {text}"));
        if let Some(window) = weak.upgrade() {
            window.set_pick_view(PickView {
                path_edited: true,
                can_confirm: false,
                hint: "Enter로 경로를 먼저 확인하세요".into(),
                ..pick_view()
            });
        }
    });
    window.set_pick_view(pick_view());
    window.set_dialog(DialogKind::Pick);
    slint::platform::update_timers_and_animations();
    let field = || labelled(&window, "대상 경로", AccessibleRole::TextInput).remove(0);
    assert_eq!(
        field().accessible_value().as_deref(),
        Some("/home/me/작업/백업")
    );

    field().set_accessible_value("/home/me/다른 곳");
    slint::platform::update_timers_and_animations();
    labelled(&window, "여기로 이동", AccessibleRole::Button)[0].invoke_accessible_default_action();
    assert_eq!(calls.borrow().as_slice(), ["edited /home/me/다른 곳"]);
    assert_eq!(
        labelled(
            &window,
            "Enter로 경로를 먼저 확인하세요",
            AccessibleRole::Text
        )
        .len(),
        1
    );
    assert_eq!(
        field().accessible_value().as_deref(),
        Some("/home/me/다른 곳")
    );

    // Another folder chosen (or the path found): the field shows the target again.
    window.set_pick_view(pick_view());
    slint::platform::update_timers_and_animations();
    assert_eq!(
        field().accessible_value().as_deref(),
        Some("/home/me/작업/백업")
    );
    labelled(&window, "여기로 이동", AccessibleRole::Button)[0].invoke_accessible_default_action();
    assert_eq!(
        calls.borrow().as_slice(),
        ["edited /home/me/다른 곳", "confirmed"]
    );
}

#[test]
fn an_edit_that_brings_the_target_back_keeps_the_cursor() {
    let window = window();
    let edits = Rc::new(RefCell::new(Vec::new()));
    // Rust, as the controller does: the field is pending while it says another path.
    let record = Rc::clone(&edits);
    let weak = window.as_weak();
    window.on_pick_path_edited(move |text| {
        record.borrow_mut().push(text.to_string());
        if let Some(window) = weak.upgrade() {
            window.set_pick_view(PickView {
                path_edited: text != "/home/me/작업/백업",
                ..pick_view()
            });
        }
    });
    window.set_pick_view(pick_view());
    window.set_dialog(DialogKind::Pick);
    slint::platform::update_timers_and_animations();
    let field = || labelled(&window, "대상 경로", AccessibleRole::TextInput).remove(0);
    field().mock_single_click(slint::platform::PointerEventButton::Left);

    // A character typed at the start and taken back: the field says the target again, and the
    // cursor stays where the typing was.
    // Left arrows rather than Home: on macOS Home does not move a text cursor.
    for _ in "/home/me/작업/백업".chars() {
        chord(&window, &[&char::from(Key::LeftArrow).to_string()]);
    }
    chord(&window, &["x"]);
    chord(&window, &[&char::from(Key::Backspace).to_string()]);
    slint::platform::update_timers_and_animations();
    chord(&window, &["y"]);
    assert_eq!(
        edits.borrow().as_slice(),
        [
            "x/home/me/작업/백업",
            "/home/me/작업/백업",
            "y/home/me/작업/백업"
        ]
    );
}

/// An input method's update of the text being composed (a Korean syllable not yet complete):
/// the text input shows `preedit` at its cursor, outside its `text`.
fn compose(window: &MainWindow, preedit: &str) {
    window
        .window()
        .dispatch_event(WindowEvent::internal(InternalKeyEvent {
            event_type: KeyEventType::UpdateComposition,
            preedit_text: preedit.into(),
            ..InternalKeyEvent::default()
        }));
}

/// The input method's end of a composition with `text`, which goes into the text input.
fn commit_composition(window: &MainWindow, text: &str) {
    let mut event = InternalKeyEvent {
        event_type: KeyEventType::CommitComposition,
        ..InternalKeyEvent::default()
    };
    event.key_event.text = text.into();
    window.window().dispatch_event(WindowEvent::internal(event));
}

#[test]
fn a_name_being_composed_holds_the_button() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&calls);
    window.on_pick_confirmed(move || record.borrow_mut().push("confirmed".to_owned()));
    let record = Rc::clone(&calls);
    window.on_pick_new_folder(move || record.borrow_mut().push("new folder".to_owned()));
    let record = Rc::clone(&calls);
    window.on_pick_path_edited(move |text| record.borrow_mut().push(format!("edited {text}")));
    let record = Rc::clone(&calls);
    window.on_pick_path_entered(move |text| record.borrow_mut().push(format!("entered {text}")));
    let record = Rc::clone(&calls);
    window.on_pick_enter(move || record.borrow_mut().push("enter".to_owned()));
    window.set_pick_view(PickView {
        unentered_hint: "Enter로 경로를 먼저 확인하세요".into(),
        ..pick_view()
    });
    window.set_dialog(DialogKind::Pick);
    slint::platform::update_timers_and_animations();
    let field = || labelled(&window, "대상 경로", AccessibleRole::TextInput).remove(0);
    let confirm = || labelled(&window, "여기로 이동", AccessibleRole::Button).remove(0);
    // The picker's, after the toolbar's under the scrim.
    let new_folder = || {
        labelled(&window, "새 폴더", AccessibleRole::Button)
            .pop()
            .expect("the picker's 새 폴더")
    };
    let hint = || {
        labelled(
            &window,
            "Enter로 경로를 먼저 확인하세요",
            AccessibleRole::Text,
        )
        .len()
    };
    field().mock_single_click(slint::platform::PointerEventButton::Left);
    assert_eq!(confirm().accessible_enabled(), Some(true));

    // While a syllable is composed the field's text is still the old target's path, and Rust
    // has heard of no edit: the buttons are off, so a click cannot send the items there, nor
    // can Enter (on the target's path it confirms).
    compose(&window, "ㄷ");
    slint::platform::update_timers_and_animations();
    assert_eq!(
        field().accessible_value().as_deref(),
        Some("/home/me/작업/백업")
    );
    assert_eq!(confirm().accessible_enabled(), Some(false));
    assert_eq!(new_folder().accessible_enabled(), Some(false));
    assert_eq!(hint(), 1);
    confirm().invoke_accessible_default_action();
    new_folder().invoke_accessible_default_action();
    chord(&window, &[&char::from(Key::Return).to_string()]);
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());
    // Nor in the filter field, where Enter confirms too.
    let filter = labelled(&window, "폴더 이름으로 거르기", AccessibleRole::TextInput).remove(0);
    filter.mock_single_click(slint::platform::PointerEventButton::Left);
    compose(&window, "ㅂ");
    chord(&window, &[&char::from(Key::Return).to_string()]);
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());
    compose(&window, "");
    field().mock_single_click(slint::platform::PointerEventButton::Left);
    compose(&window, "ㄷ");
    slint::platform::update_timers_and_animations();

    // The input method cancels the composition: the path is the target's again, as Rust has it.
    compose(&window, "");
    slint::platform::update_timers_and_animations();
    assert_eq!(
        field().accessible_value().as_deref(),
        Some("/home/me/작업/백업")
    );
    assert_eq!(confirm().accessible_enabled(), Some(true));
    assert_eq!(new_folder().accessible_enabled(), Some(true));
    assert_eq!(hint(), 0);
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());

    // Once a syllable is committed, the edit reaches Rust, which holds the button from then
    // on (`a_path_typed_and_not_entered_holds_the_button`).
    compose(&window, "ㄷ");
    commit_composition(&window, "다");
    slint::platform::update_timers_and_animations();
    assert_eq!(calls.borrow().as_slice(), ["edited /home/me/작업/백업다"]);
    assert_eq!(hint(), 0);
}

#[test]
fn a_folder_the_picker_cannot_take_is_not_selectable_for_screen_readers() {
    let window = window();
    let toggled = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&toggled);
    window.on_pick_tree_toggled(move |index| record.borrow_mut().push(index));
    let rows = Rc::new(slint::VecModel::from(vec![
        rf_ui::TreeRow {
            label: "보관함".into(),
            ..rf_ui::TreeRow::default()
        },
        rf_ui::TreeRow {
            label: "assets".into(),
            depth: 1,
            dimmed: true,
            expandable: true,
            info: "옮기는 항목이라 고를 수 없음".into(),
            ..rf_ui::TreeRow::default()
        },
    ]));
    window.set_pick_view(PickView {
        tree: ModelRc::from(rows),
        ..pick_view()
    });
    window.set_dialog(DialogKind::Pick);
    slint::platform::update_timers_and_animations();

    let row = |label: &str| {
        let found = labelled(&window, label, AccessibleRole::ListItem);
        assert_eq!(found.len(), 1, "one row {label}");
        found.into_iter().next().expect("a row")
    };
    // Neither is disabled (unset is enabled).
    assert_ne!(row("보관함").accessible_enabled(), Some(false));
    assert_eq!(row("보관함").accessible_item_selectable(), Some(true));
    // It cannot be chosen, but a screen reader can still open it for the folders below.
    assert_ne!(row("assets").accessible_enabled(), Some(false));
    assert_eq!(row("assets").accessible_item_selectable(), Some(false));
    row("assets").invoke_accessible_expand_action();
    assert_eq!(toggled.borrow().as_slice(), [1]);
}

/// What a drag's callbacks told: the payload's id, where, which pane, which row or tab, and
/// the action asked for.
type DropCall = (u64, DropPlace, i32, i32, DragAction);

/// The centre of the one element labelled `label` with role `role`.
fn centre_of(window: &MainWindow, label: &str, role: AccessibleRole) -> LogicalPosition {
    let found = labelled(window, label, role);
    assert_eq!(found.len(), 1, "one {label}");
    centre(&found[0])
}

fn centre(element: &ElementHandle) -> LogicalPosition {
    let (at, size) = (element.absolute_position(), element.size());
    LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
}

fn pointer(window: &MainWindow, event: &str, position: LogicalPosition) {
    let button = slint::platform::PointerEventButton::Left;
    window.window().dispatch_event(match event {
        "press" => WindowEvent::PointerPressed { position, button },
        "release" => WindowEvent::PointerReleased { position, button },
        _ => WindowEvent::PointerMoved { position },
    });
}

/// A press at `from` and a drag past the threshold toward `to`, where it stays.
fn drag_to(window: &MainWindow, from: LogicalPosition, to: LogicalPosition) {
    pointer(window, "press", from);
    for step in [10.0, 20.0, 30.0] {
        pointer(window, "move", LogicalPosition::new(from.x + step, from.y));
    }
    pointer(window, "move", to);
}

/// What the window's drag callbacks were called with.
#[derive(Default)]
struct DragCalls {
    /// Presses and releases of rows: which, pane and row.
    rows: RefCell<Vec<(&'static str, i32, i32)>>,
    checks: RefCell<Vec<DropCall>>,
    drops: RefCell<Vec<DropCall>>,
}

/// A window whose left pane lists the folder `docs` and the file `x.txt`, whose right pane
/// lists the folder `백업` and the file `y.txt`, and whose tree has the place `홈`, with the
/// payload of drag 42. Its callbacks answer as the controller does when the items are in the
/// left pane's folder: the left pane's list takes nothing, every other place takes them,
/// and the label names the place.
fn drag_window() -> (MainWindow, Rc<DragCalls>, [PaneModels; 2], Rc<TreeRows>) {
    let window = window();
    let left = listed_pane(vec![
        entry("docs", EntryKind::Dir, 0),
        entry("x.txt", EntryKind::File, 10),
    ]);
    let right = listed_pane(vec![
        entry("백업", EntryKind::Dir, 0),
        entry("y.txt", EntryKind::File, 10),
    ]);
    let models = [PaneModels::default(), PaneModels::default()];
    window.set_left_pane(models[0].view(&left, true));
    window.set_right_pane(models[1].view(&right, false));
    let home = VPath::local(std::env::temp_dir().join("home")).expect("a local path");
    let mut tree = Tree::new(VisibilityFilter::default());
    tree.set_roots([("홈".to_owned(), home.clone())]);
    let places = [Place {
        label: "홈".to_owned(),
        path: home,
        kind: PlaceKind::Favorite(KnownFolder::Home),
        space: None,
    }];
    let tree_rows = Rc::new(TreeRows::default());
    tree_rows.sync(&tree, &places);
    window.set_tree(ModelRc::from(Rc::clone(&tree_rows)));
    window.set_drag_data(ui_bridge::drag_payload(42));

    let calls = Rc::new(DragCalls::default());
    let record = Rc::clone(&calls);
    window.on_row_clicked(move |side, i, _, _| record.rows.borrow_mut().push(("press", side, i)));
    let record = Rc::clone(&calls);
    window.on_row_released(move |side, i| record.rows.borrow_mut().push(("release", side, i)));
    let record = Rc::clone(&calls);
    let weak = window.as_weak();
    window.on_drop_check(move |data, place, side, i, action| {
        let call = (ui_bridge::drag_id(&data), place, side, i, action);
        record.checks.borrow_mut().push(call);
        if side == 0 && place == DropPlace::List {
            return DragAction::None;
        }
        if let Some(window) = weak.upgrade() {
            window.set_drag_label(format!("{place:?} {i}").into());
        }
        action
    });
    let record = Rc::clone(&calls);
    window.on_drop_items(move |data, place, side, i, action| {
        let call = (ui_bridge::drag_id(&data), place, side, i, action);
        record.drops.borrow_mut().push(call);
        action
    });
    (window, calls, models, tree_rows)
}

#[test]
fn a_row_dragged_onto_the_other_pane_drops_there() {
    let (window, calls, _models, _tree) = drag_window();
    let x_txt = centre_of(&window, "x.txt", AccessibleRole::ListItem);
    let backup = centre_of(&window, "백업", AccessibleRole::ListItem);
    let y_txt = centre_of(&window, "y.txt", AccessibleRole::ListItem);
    // Below the right pane's rows.
    let right_empty = LogicalPosition::new(y_txt.x, y_txt.y + 200.0);
    let last_check = || calls.checks.borrow().last().copied();

    // A click (no drag) presses and releases the row.
    pointer(&window, "move", x_txt);
    pointer(&window, "press", x_txt);
    pointer(&window, "release", x_txt);
    assert_eq!(
        calls.rows.borrow_mut().drain(..).collect::<Vec<_>>(),
        [("press", 0, 1), ("release", 0, 1)]
    );

    // A drag over the other pane: its folder row, then its list below the rows. The payload
    // is the one Rust made; no modifier asks for a move.
    drag_to(&window, x_txt, backup);
    assert_eq!(
        last_check(),
        Some((42, DropPlace::List, 1, 0, DragAction::Move))
    );
    // While a place takes it, the label says what the drop would do.
    assert_eq!(labelled(&window, "List 0", AccessibleRole::Text).len(), 1);
    pointer(&window, "move", right_empty);
    assert_eq!(
        last_check(),
        Some((42, DropPlace::List, 1, -1, DragAction::Move))
    );
    // Ctrl asks for a copy, from the moment it is pressed.
    let control = char::from(Key::Control).to_string();
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: control.clone().into(),
    });
    assert_eq!(
        last_check(),
        Some((42, DropPlace::List, 1, -1, DragAction::Copy))
    );
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: control.into(),
    });
    assert_eq!(
        last_check(),
        Some((42, DropPlace::List, 1, -1, DragAction::Move))
    );
    pointer(&window, "release", right_empty);
    assert_eq!(
        calls.drops.borrow_mut().drain(..).collect::<Vec<_>>(),
        [(42, DropPlace::List, 1, -1, DragAction::Move)]
    );
    // The drag took the release: no click of the row, and the label is gone.
    assert_eq!(
        calls.rows.borrow_mut().drain(..).collect::<Vec<_>>(),
        [("press", 0, 1)]
    );
    assert_eq!(labelled(&window, "List -1", AccessibleRole::Text).len(), 0);
}

#[test]
fn nothing_drops_where_rust_refuses_and_tabs_and_the_tree_take_drops() {
    let (window, calls, _models, _tree) = drag_window();
    let x_txt = centre_of(&window, "x.txt", AccessibleRole::ListItem);
    let docs = centre_of(&window, "docs", AccessibleRole::ListItem);

    // Over the pane the items are in nothing takes them, and a release there drops nothing.
    drag_to(&window, x_txt, docs);
    assert_eq!(
        calls.checks.borrow().last().copied(),
        Some((42, DropPlace::List, 0, 0, DragAction::Move))
    );
    pointer(&window, "release", docs);
    assert_eq!(calls.drops.borrow().as_slice(), []);

    // A tab of the other pane (both show the folder `listed`: it is the one further right),
    // and a row of the tree.
    let tabs = labelled(&window, "listed", AccessibleRole::Tab);
    assert_eq!(tabs.len(), 2, "a tab in each pane");
    let right_tab = tabs
        .iter()
        .map(centre)
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .expect("two tabs");
    let home = centre_of(&window, "홈", AccessibleRole::ListItem);
    for (target, place, side) in [(right_tab, DropPlace::Tab, 1), (home, DropPlace::Tree, 0)] {
        drag_to(&window, x_txt, target);
        pointer(&window, "release", target);
        assert_eq!(
            calls.drops.borrow_mut().drain(..).collect::<Vec<_>>(),
            [(42, place, side, 0, DragAction::Move)]
        );
    }
}

#[test]
fn a_drag_carries_the_payload_its_press_made() {
    let (window, calls, _models, _tree) = drag_window();
    // As the controller does: the press readies a drag of the items it selects, and the
    // window has its id before the drag starts.
    let weak = window.as_weak();
    let record = Rc::clone(&calls);
    window.on_row_clicked(move |side, i, _, _| {
        record.rows.borrow_mut().push(("press", side, i));
        if let Some(window) = weak.upgrade() {
            let id = 100 + u64::try_from(i).expect("a row");
            window.set_drag_data(ui_bridge::drag_payload(id));
        }
    });
    let x_txt = centre_of(&window, "x.txt", AccessibleRole::ListItem);
    let docs = centre_of(&window, "docs", AccessibleRole::ListItem);
    let backup = centre_of(&window, "백업", AccessibleRole::ListItem);
    let last_id = || calls.checks.borrow().last().map(|call| call.0);

    drag_to(&window, x_txt, backup);
    assert_eq!(last_id(), Some(101));
    pointer(&window, "release", backup);
    drag_to(&window, docs, backup);
    assert_eq!(last_id(), Some(100));
    pointer(&window, "release", backup);
    assert_eq!(
        calls
            .drops
            .borrow()
            .iter()
            .map(|call| call.0)
            .collect::<Vec<_>>(),
        [101, 100]
    );
}

#[test]
fn a_double_click_on_a_row_opens_it() {
    let (window, calls, _models, _tree) = drag_window();
    let opened = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&opened);
    window.on_row_activated(move |side, i| record.borrow_mut().push((side, i)));
    let y_txt = centre_of(&window, "y.txt", AccessibleRole::ListItem);

    // Two clicks at once (the testing backend's time stands still) in the right pane's
    // second row: the drag area around the row lets them through.
    pointer(&window, "move", y_txt);
    for _ in 0..2 {
        pointer(&window, "press", y_txt);
        pointer(&window, "release", y_txt);
    }
    assert_eq!(opened.borrow().as_slice(), [(1, 1)]);
    assert_eq!(
        calls.rows.borrow().as_slice(),
        [
            ("press", 1, 1),
            ("release", 1, 1),
            ("press", 1, 1),
            ("release", 1, 1)
        ]
    );
    assert!(calls.drops.borrow().is_empty());
}
