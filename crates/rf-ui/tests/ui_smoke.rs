//! Headless checks of the Slint screens (no window system needed).

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
// The input method's composition has no public event; Slint's own backends send it this way.
use i_slint_core::input::{InternalKeyEvent, KeyEventType};
use rf_core::{Entry, EntryKind, Os, VPath};
use rf_platform::{DiskSpace, KnownFolder};
use rf_ui::loader::LoadEvent;
use rf_ui::pane::Pane;
use rf_ui::places::{Place, PlaceKind};
use rf_ui::selection::Selection;
use rf_ui::state::{Column, FolderView, MenuFacts, ShownColumn, ViewMode};
use rf_ui::tree::Tree;
use rf_ui::ui_bridge::{self, FileRows, PaneModels, TreeRows};
use rf_ui::{
    ConfirmView, ConflictView, DialogKind, DropPlace, FactRow, FolderMenuRow, FolderMenuView,
    HashRowView, JobRowView, MainWindow, MessageView, NameView, NoticeWindow, PaneView,
    PatternView, PickTarget, PickView, PreflightView, PreviewKind, PreviewView, PropertiesView,
    RecoveryView, Theme,
};
use rf_vfs::VisibilityFilter;
use slint::language::DragAction;
use slint::platform::{Key, WindowEvent};
use slint::{Color, ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc, SharedString};

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
fn the_notice_window_shows_the_theme_it_is_given() {
    i_slint_backend_testing::init_no_event_loop();
    for (choice, dark) in [(1, false), (2, true)] {
        let notice = NoticeWindow::new().expect("NoticeWindow builds");
        notice.set_theme_choice(choice);
        notice.show().expect("the notice shows");
        assert_eq!(notice.global::<Theme<'_>>().get_dark(), dark, "{choice}");
        notice.hide().expect("the notice hides");
    }
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
    window.on_key(move |text, ctrl, shift, alt, meta| {
        record
            .borrow_mut()
            .push((text.to_string(), ctrl, shift, alt, meta));
        true
    });

    assert!(window.invoke_key("a".into(), true, false, false, false));
    assert_eq!(
        seen.borrow().as_slice(),
        [("a".to_owned(), true, false, false, false)]
    );

    // A key pressed in the window reaches the callback through the window's key scope.
    let down = char::from(Key::DownArrow).to_string();
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: down.clone().into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: down.clone().into(),
    });
    assert_eq!(
        seen.borrow().last(),
        Some(&(down, false, false, false, false))
    );
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
    window.on_key(move |text, ctrl, _, _, _| {
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

/// The end of the name of each `›` of a path row: "‘docs’ 하위 폴더".
const SUBFOLDERS: &str = "하위 폴더";

/// A list of the address bar with `rows`, the cursor on row `cursor` (-1 for none) and `note`
/// under them.
fn folder_menu(rows: &[&str], cursor: i32, note: &str) -> FolderMenuView {
    let rows: Vec<FolderMenuRow> = rows
        .iter()
        .map(|row| FolderMenuRow {
            name: (*row).into(),
            link: false,
        })
        .collect();
    FolderMenuView {
        open: true,
        rows: ModelRc::new(slint::VecModel::from(rows)),
        cursor,
        note: note.into(),
        pending: false,
    }
}

/// Presses and releases the left button at `(x, y)` of the window.
fn click_at(window: &MainWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
    window
        .window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
}

#[test]
fn a_pieces_chevron_asks_for_its_folders_and_the_list_takes_clicks() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = Rc::clone(&calls);
    window.on_subfolders_clicked(move |side, index| {
        record.borrow_mut().push(format!("clicked {side} {index}"));
    });
    let record = Rc::clone(&calls);
    window.on_subfolder_chosen(move |index| record.borrow_mut().push(format!("chosen {index}")));
    let record = Rc::clone(&calls);
    window.on_subfolders_dismissed(move || record.borrow_mut().push("dismissed".to_owned()));
    let pane = listed_pane(vec![]);
    let models = PaneModels::default();
    let view = models.view(&pane, true);
    let labels: Vec<String> = view.crumbs.iter().map(|crumb| crumb.label.into()).collect();
    let pieces = labels.len();
    window.set_right_pane(view);
    // A row too long for the pane puts its last piece at its end as the window takes in an
    // event (its pieces' places come out of the layout): the pointer comes over the window
    // first, as it does before any click.
    pointer(&window, "move", LogicalPosition::new(1.0, 1.0));
    // The buttons of the right pane's path row, as it shows them: its pieces (the "…" one
    // named for the folder it stands for) and their `›`s.
    let path_row = || {
        let right = labelled(&window, "오른쪽 패널", AccessibleRole::Groupbox).remove(0);
        let row = right
            .query_descendants()
            .match_accessible_role(AccessibleRole::Groupbox)
            .match_predicate(|group| group.accessible_label().as_deref() == Some("경로"))
            .find_first()
            .expect("the path row");
        let (chevrons, shown): (Vec<_>, Vec<_>) = row
            .query_descendants()
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
            .into_iter()
            .partition(|button| {
                button
                    .accessible_label()
                    .is_some_and(|label| label.ends_with(SUBFOLDERS))
            });
        (chevrons, shown)
    };
    let name = |button: &ElementHandle| button.accessible_label().unwrap_or_default().to_string();
    // Each piece the row shows has a `›` after it, named for its folder, the last one for the
    // pane's own. A long path (the temporary folder of macOS or Windows) shows only its last
    // pieces, after a "…" piece that stands for the folder before them.
    let (chevrons, shown) = path_row();
    assert!(
        !shown.is_empty() && shown.len() <= pieces,
        "{} of {pieces}",
        shown.len()
    );
    let first = pieces - shown.len();
    assert_eq!(shown.iter().map(name).collect::<Vec<_>>(), labels[first..]);
    assert_eq!(
        chevrons.iter().map(name).collect::<Vec<_>>(),
        labels[first..]
            .iter()
            .map(|label| format!("‘{label}’ {SUBFOLDERS}"))
            .collect::<Vec<_>>()
    );
    let last = path_row().0.pop().expect("the last piece's chevron");
    assert_eq!(
        last.accessible_label().as_deref(),
        Some("‘listed’ 하위 폴더")
    );
    last.mock_single_click(slint::platform::PointerEventButton::Left);
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        [format!("clicked 1 {}", pieces - 1)]
    );

    // The list Rust gives shows under it, with the keys' row and the note; a click on a row
    // chooses it.
    window.set_subfolders(folder_menu(&["docs", "src"], 1, "외 3개"));
    // A page of it is the rows it shows.
    assert_eq!(window.get_subfolders_page(), 2);
    let row = |name: &str| labelled(&window, name, AccessibleRole::ListItem).remove(0);
    assert_eq!(row("docs").accessible_item_selected(), Some(false));
    assert_eq!(row("src").accessible_item_selected(), Some(true));
    assert_eq!(labelled(&window, "외 3개", AccessibleRole::Text).len(), 1);
    let (chevron, docs) = (last.absolute_position(), row("docs").absolute_position());
    assert!(
        docs.y > chevron.y,
        "the list is under the chevron: {docs:?} {chevron:?}"
    );
    row("src").mock_single_click(slint::platform::PointerEventButton::Left);
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["chosen 1"]
    );

    // A press anywhere else closes it.
    click_at(&window, 40.0, 780.0);
    click_at(&window, 1000.0, 600.0);
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["dismissed", "dismissed"]
    );
    // A long list shows ten rows, a page.
    let many: Vec<String> = (1..=25).map(|n| format!("여행 {n}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    window.set_subfolders(folder_menu(&many, -1, ""));
    assert_eq!(window.get_subfolders_page(), 10);
    // So do the keys leaving the panes (here for the filter box).
    window.invoke_focus_filter();
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["dismissed"]
    );
    // Closed, it takes no click.
    window.set_subfolders(FolderMenuView::default());
    click_at(&window, 1000.0, 600.0);
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());
}

#[test]
fn the_path_field_asks_for_completions_and_takes_the_lists_keys() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = Rc::clone(&calls);
    window.on_path_edited(move |side, text| {
        record.borrow_mut().push(format!("edited {side} {text}"));
    });
    let record = Rc::clone(&calls);
    window.on_completion_step(move |side, delta| {
        record.borrow_mut().push(format!("step {side} {delta}"));
    });
    let record = Rc::clone(&calls);
    window.on_path_completed(move |side, text| {
        record.borrow_mut().push(format!("complete {side} {text}"));
        "/srv/docs/".into()
    });
    let record = Rc::clone(&calls);
    window.on_completion_dismissed(move |side| {
        record.borrow_mut().push(format!("dismissed {side}"));
    });
    let record = Rc::clone(&calls);
    window.on_completion_chosen(move |side, index| {
        record.borrow_mut().push(format!("chosen {side} {index}"));
    });
    let record = Rc::clone(&calls);
    window.on_path_entered(move |side, text| {
        record.borrow_mut().push(format!("entered {side} {text}"));
    });
    let record = Rc::clone(&calls);
    window.on_key(move |text, _, _, _, _| {
        record.borrow_mut().push(format!("key {text}"));
        true
    });
    let taken = || calls.borrow_mut().drain(..).collect::<Vec<_>>();
    let key = |k: Key| char::from(k).to_string();
    let (down, up, tab, escape) = (
        key(Key::DownArrow),
        key(Key::UpArrow),
        key(Key::Tab),
        key(Key::Escape),
    );
    let mut view = window.get_right_pane();
    view.path_text = "/srv".into();
    window.set_right_pane(view);
    window.invoke_edit_path(1);
    // The field that shows: the other pane's is hidden.
    let field = || labelled(&window, "경로", AccessibleRole::TextInput).remove(0);

    // The text goes to Rust as it changes (→ ends the selection at the end of the text).
    chord(&window, &[&key(Key::RightArrow)]);
    chord(&window, &["/"]);
    chord(&window, &["d"]);
    assert_eq!(taken(), ["edited 1 /srv/", "edited 1 /srv/d"]);
    // Without a list, the arrows are the field's own.
    chord(&window, &[&down]);
    assert_eq!(taken(), Vec::<String>::new());

    // With the list Rust gives: its rows show under the field, ↑↓ step through them, Tab
    // completes the text, Esc closes the list and keeps the field.
    let mut view = window.get_right_pane();
    view.completion = folder_menu(&["docs", "downloads"], -1, "");
    window.set_right_pane(view);
    assert_eq!(
        labelled(&window, "downloads", AccessibleRole::ListItem).len(),
        1
    );
    // It is as wide as the field and 6px more on either side, as far as the pane's frame, and
    // right under the field (the field takes the path row less 8px on either side, and 4px
    // above and below).
    let list = labelled(&window, "경로 자동 완성", AccessibleRole::Groupbox).remove(0);
    let row = path_row_of(&window, "오른쪽 패널");
    let (list_at, row_at) = (list.absolute_position(), row.absolute_position());
    assert!(
        (list_at.x - (row_at.x + 2.0)).abs() < 0.5,
        "{list_at:?} {row_at:?}"
    );
    assert!((list.size().width - (row.size().width - 4.0)).abs() < 0.5);
    let under = row_at.y + row.size().height - 4.0 + 2.0;
    assert!((list_at.y - under).abs() < 0.5, "{list_at:?} {row_at:?}");
    chord(&window, &[&down]);
    chord(&window, &[&up]);
    chord(&window, &[&tab]);
    assert_eq!(taken(), ["step 1 1", "step 1 -1", "complete 1 /srv/d"]);
    assert_eq!(field().accessible_value().as_deref(), Some("/srv/docs/"));
    chord(&window, &[&escape]);
    assert_eq!(taken(), ["dismissed 1"]);
    chord(&window, &["x"]);
    assert_eq!(taken(), ["edited 1 /srv/docs/x"]);
    // A click on a row chooses it, and the field closes.
    labelled(&window, "downloads", AccessibleRole::ListItem)
        .remove(0)
        .mock_single_click(slint::platform::PointerEventButton::Left);
    assert_eq!(taken(), ["chosen 1 1"]);
    chord(&window, &["y"]);
    assert_eq!(taken(), ["key y"]);

    // Rust closed the list: Esc closes the field, which drops the list too.
    let mut view = window.get_right_pane();
    view.completion = FolderMenuView::default();
    window.set_right_pane(view);
    window.invoke_edit_path(1);
    chord(&window, &[&escape]);
    chord(&window, &["z"]);
    assert_eq!(taken(), ["dismissed 1", "key z"]);
}

/// The path row of the pane named `pane` ("왼쪽 패널" or "오른쪽 패널").
fn path_row_of(window: &MainWindow, pane: &str) -> ElementHandle {
    labelled(window, pane, AccessibleRole::Groupbox)
        .remove(0)
        .query_descendants()
        .match_accessible_role(AccessibleRole::Groupbox)
        .match_predicate(|group| group.accessible_label().as_deref() == Some("경로"))
        .find_first()
        .expect("the path row")
}

#[test]
fn a_tab_before_the_first_completions_keeps_the_field_and_waits() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = Rc::clone(&calls);
    window.on_path_completed(move |side, text| {
        record.borrow_mut().push(format!("complete {side} {text}"));
        SharedString::new()
    });
    let record = Rc::clone(&calls);
    window.on_completion_step(move |side, delta| {
        record.borrow_mut().push(format!("step {side} {delta}"));
    });
    let record = Rc::clone(&calls);
    window.on_key(move |text, _, _, _, _| {
        record.borrow_mut().push(format!("key {text}"));
        true
    });
    let record = Rc::clone(&calls);
    window.on_path_edited(move |side, text| {
        record.borrow_mut().push(format!("edited {side} {text}"));
    });
    let taken = || calls.borrow_mut().drain(..).collect::<Vec<_>>();
    let key = |k: Key| char::from(k).to_string();
    let mut view = window.get_left_pane();
    view.path_text = "/srv/d".into();
    view.completion = FolderMenuView {
        pending: true,
        ..FolderMenuView::default()
    };
    window.set_left_pane(view);
    window.invoke_edit_path(0);
    chord(&window, &[&key(Key::RightArrow)]);
    // Rust waits for the first names: Tab goes to it and stays in the field, ↓ is the field's.
    chord(&window, &[&key(Key::Tab)]);
    chord(&window, &[&key(Key::DownArrow)]);
    chord(&window, &["x"]);
    assert_eq!(taken(), ["complete 0 /srv/d", "edited 0 /srv/dx"]);
    // The names come, and the text the Tab waited for fills the field.
    window.invoke_fill_path(0, "/srv/docs/".into());
    let field = labelled(&window, "경로", AccessibleRole::TextInput).remove(0);
    assert_eq!(field.accessible_value().as_deref(), Some("/srv/docs/"));
    chord(&window, &["y"]);
    assert_eq!(taken(), ["edited 0 /srv/docs/y"]);
    // A closed field takes no text.
    let mut view = window.get_left_pane();
    view.completion = FolderMenuView::default();
    window.set_left_pane(view);
    chord(&window, &[&key(Key::Escape)]);
    window.invoke_fill_path(0, "/elsewhere".into());
    chord(&window, &["z"]);
    assert_eq!(taken(), ["key z"]);
}

#[test]
fn esc_while_a_syllable_is_composed_closes_and_carries_out_nothing() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = Rc::clone(&calls);
    window
        .on_completion_dismissed(move |side| record.borrow_mut().push(format!("dismissed {side}")));
    let record = Rc::clone(&calls);
    window.on_path_entered(move |_, text| record.borrow_mut().push(format!("entered {text}")));
    let record = Rc::clone(&calls);
    window.on_path_completed(move |_, text| {
        record.borrow_mut().push(format!("complete {text}"));
        SharedString::new()
    });
    let record = Rc::clone(&calls);
    window.on_key(move |text, _, _, _, _| {
        record.borrow_mut().push(format!("key {text}"));
        true
    });
    let taken = || calls.borrow_mut().drain(..).collect::<Vec<_>>();
    let escape = char::from(Key::Escape).to_string();
    let mut view = window.get_right_pane();
    view.path_text = "/srv/".into();
    view.completion = folder_menu(&["docs"], 0, "");
    window.set_right_pane(view);
    window.invoke_edit_path(1);
    // Esc while a syllable is composed closes the list, and the field stays.
    compose(&window, "ㄷ");
    chord(&window, &[&escape]);
    assert_eq!(taken(), ["dismissed 1"]);
    // Rust closed the list: Esc closes the field, and nothing is entered or completed.
    let mut view = window.get_right_pane();
    view.completion = FolderMenuView::default();
    window.set_right_pane(view);
    compose(&window, "ㄷ");
    chord(&window, &[&escape]);
    assert_eq!(taken(), ["dismissed 1"]);
    chord(&window, &["q"]);
    assert_eq!(taken(), ["key q"]);
}

#[test]
fn the_completion_list_fits_a_short_pane_and_shows_its_cursor() {
    let window = window();
    // One pane above the other, the lower one as short as a pane gets.
    window.set_stacked(true);
    window.set_split_share(0.9);
    let names: Vec<String> = (1..=20).map(|n| format!("폴더 {n}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    for (side, pane) in [(1, "오른쪽 패널"), (0, "왼쪽 패널")] {
        let set = |cursor: i32| {
            let mut view = if side == 1 {
                window.get_right_pane()
            } else {
                window.get_left_pane()
            };
            view.path_text = "/srv/".into();
            view.completion = folder_menu(&names, cursor, "");
            if side == 1 {
                window.set_right_pane(view);
            } else {
                window.set_left_pane(view);
            }
        };
        set(-1);
        window.invoke_edit_path(side);
        // In the lower pane the list shows (as the window draws it), then the keys go down it to
        // its last row; in the upper one it is made with its cursor on that row already.
        if side == 1 {
            assert_eq!(
                labelled(&window, "경로 자동 완성", AccessibleRole::Groupbox).len(),
                1,
                "{pane}"
            );
        }
        set(19);
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
        }
        let height = window
            .window()
            .size()
            .to_logical(window.window().scale_factor())
            .height;
        let row = path_row_of(&window, pane);
        let field_top = row.absolute_position().y + 4.0;
        let field_bottom = row.absolute_position().y + row.size().height - 4.0;
        let list = labelled(&window, "경로 자동 완성", AccessibleRole::Groupbox).remove(0);
        let (top, bottom) = (
            list.absolute_position().y,
            list.absolute_position().y + list.size().height,
        );
        // In the window, and clear of the field.
        assert!(
            top >= 0.0 && bottom <= height,
            "{pane}: {top} {bottom} {height}"
        );
        assert!(
            top >= field_bottom || bottom <= field_top,
            "{pane}: {top} {bottom}"
        );
        // The cursor's row shows whole inside it.
        let last = labelled(&window, "폴더 20", AccessibleRole::ListItem);
        let last = last
            .iter()
            .find(|row| row.accessible_item_selected() == Some(true))
            .unwrap_or_else(|| panic!("{pane}: the cursor's row is made"));
        let (at, size) = (last.absolute_position(), last.size());
        assert!(
            at.y >= top && at.y + size.height <= bottom,
            "{pane}: row {} to {} in {top} to {bottom}",
            at.y,
            at.y + size.height
        );
        // The field closes for the other pane's turn.
        chord(&window, &[&char::from(Key::Escape).to_string()]);
        let mut view = if side == 1 {
            window.get_right_pane()
        } else {
            window.get_left_pane()
        };
        view.completion = FolderMenuView::default();
        if side == 1 {
            window.set_right_pane(view);
        } else {
            window.set_left_pane(view);
        }
        chord(&window, &[&char::from(Key::Escape).to_string()]);
    }
}

#[test]
fn the_subfolder_list_opens_above_its_chevron_when_only_there_it_fits_whole() {
    let window = window();
    window.set_stacked(true);
    window.set_split_share(0.9);
    let pane = listed_pane(vec![]);
    let models = PaneModels::default();
    window.set_right_pane(models.view(&pane, true));
    pointer(&window, "move", LogicalPosition::new(1.0, 1.0));
    let chevron = path_row_of(&window, "오른쪽 패널")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Button)
        .find_all()
        .into_iter()
        .rfind(|button| {
            button
                .accessible_label()
                .is_some_and(|label| label.ends_with(SUBFOLDERS))
        })
        .expect("the last piece's chevron");
    chevron.mock_single_click(slint::platform::PointerEventButton::Left);
    let (top, bottom) = (
        chevron.absolute_position().y,
        chevron.absolute_position().y + chevron.size().height,
    );
    let place = |count: usize| {
        let names: Vec<String> = (1..=count).map(|n| format!("여행 {n}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        window.set_subfolders(folder_menu(&names, -1, ""));
        let list = labelled(&window, "하위 폴더", AccessibleRole::Groupbox).remove(0);
        (
            list.absolute_position().y,
            list.absolute_position().y + list.size().height,
        )
    };
    // Three rows fit under the `›` of the short lower pane: the list goes there.
    let (list_top, _) = place(3);
    assert!(list_top >= bottom, "{list_top} {bottom}");
    // Eight do not, and fit above it: the list goes above, whole.
    let (_, list_bottom) = place(8);
    assert!(list_bottom <= top, "{list_bottom} {top}");
    assert_eq!(window.get_subfolders_page(), 8);
}

#[test]
fn no_completion_key_acts_while_a_syllable_is_composed() {
    let window = window();
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = Rc::clone(&calls);
    window.on_completion_step(move |_, delta| record.borrow_mut().push(format!("step {delta}")));
    let record = Rc::clone(&calls);
    window.on_path_completed(move |_, text| {
        record.borrow_mut().push(format!("complete {text}"));
        SharedString::new()
    });
    let record = Rc::clone(&calls);
    window.on_path_entered(move |_, text| record.borrow_mut().push(format!("entered {text}")));
    let key = |k: Key| char::from(k).to_string();
    let mut view = window.get_left_pane();
    view.path_text = "/srv/".into();
    view.completion = folder_menu(&["docs"], 0, "");
    window.set_left_pane(view);
    window.invoke_edit_path(0);
    chord(&window, &[&key(Key::RightArrow)]);
    // A syllable is being composed: Tab, the arrows and Enter leave it to the input method (the
    // arrows move the field's own cursor, ↑ to the start and ↓ back to the end).
    compose(&window, "ㄷ");
    for k in [Key::Tab, Key::UpArrow, Key::DownArrow, Key::Return] {
        chord(&window, &[&key(k)]);
    }
    assert_eq!(calls.borrow().as_slice(), Vec::<String>::new());
    // Once it is committed, they act again.
    commit_composition(&window, "다");
    chord(&window, &[&key(Key::DownArrow)]);
    chord(&window, &[&key(Key::Tab)]);
    chord(&window, &[&key(Key::Return)]);
    assert_eq!(
        calls.borrow().as_slice(),
        ["step 1", "complete /srv/다", "entered /srv/다"]
    );
}

#[test]
fn a_long_path_starts_with_the_ellipsis_and_its_folders_follow() {
    let window = window();
    // Names that together are too wide for a pane in any font, each narrow enough to show
    // beside the last one (a CI runner may have no font for Hangul, which would draw Korean
    // names narrow or not at all).
    let mut path = std::env::temp_dir();
    for name in [
        "project papers of the team",
        "first half of the year 2026",
        "drafts of the yearly report",
        "notes sorted by department",
        "copies for the final review",
        "the folder before the last",
        "last",
    ] {
        path.push(name);
    }
    let folder = VPath::local(&path).expect("a local path");
    let crumbs = ui_bridge::crumbs(&rf_ui::pane::ancestors(&folder));
    let (pane, _) = Pane::new(folder, VisibilityFilter::default());
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));
    // As the event loop does before it draws: the pieces are made (a look at the window makes
    // them here), then their change handlers run.
    assert!(!labelled(&window, "last", AccessibleRole::Button).is_empty());
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
    }
    let row = labelled(&window, "경로", AccessibleRole::Groupbox)
        .into_iter()
        .min_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x))
        .expect("the left pane's path row");
    let (row_x, row_width) = (row.absolute_position().x, row.size().width);
    // The pieces, without the `›` between them (each a button that lists a folder's folders).
    let mut pieces: Vec<ElementHandle> = row
        .query_descendants()
        .match_accessible_role(AccessibleRole::Button)
        .find_all()
        .into_iter()
        .filter(|piece| {
            !piece
                .accessible_label()
                .is_some_and(|label| label.ends_with(SUBFOLDERS))
        })
        .collect();
    pieces.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    let place = |piece: &ElementHandle| {
        let x = piece.absolute_position().x - row_x;
        (
            piece.accessible_label().unwrap_or_default().to_string(),
            x,
            x + piece.size().width,
        )
    };
    let shown: Vec<_> = pieces.iter().map(place).collect();
    let [ellipsis, first, .., last] = shown.as_slice() else {
        panic!("a cut path: {shown:?}");
    };
    // The path is cut: the root and the folders after it up to the "…" piece's (the nearest
    // folder out of view, which it is named for) are hidden, and the folders after it show.
    // A path that is not cut starts with the root's piece instead, named as the "…" would be
    // with the root alone out of view: here more than the root is.
    let at = |label: &str| crumbs.iter().position(|crumb| crumb.label == label);
    let hidden = at(&ellipsis.0).expect("the … piece is named for a folder of the path");
    assert!(hidden >= 1, "the path is cut after the root: {shown:?}");
    assert_eq!(at(&first.0), Some(hidden + 1), "{shown:?}");
    assert_eq!(shown.len(), crumbs.len() - hidden, "{shown:?}");
    // The "…" piece starts the row, and the first folder shown follows it after the `›`
    // (16px wide), with no gap.
    assert!(ellipsis.1 <= 8.0, "the … piece starts the row: {shown:?}");
    assert!(
        first.1 - ellipsis.2 <= 18.0,
        "the first folder shown follows the … piece: {shown:?}"
    );
    assert_eq!(last.0, "last");
    assert!(
        last.2 <= row_width,
        "the last folder shows whole: {shown:?}"
    );
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
    window.on_sort(move |side, column, then| record.borrow_mut().push((side, column, then)));
    // The heads after the name come from the pane's view.
    let models = [PaneModels::default(), PaneModels::default()];
    let pane = listed_pane(vec![entry("a.txt", EntryKind::File, 1)]);
    window.set_left_pane(models[0].view(&pane, true));
    window.set_right_pane(models[1].view(&pane, false));

    // Both panes have the heads; the left one comes first.
    let size_heads = labelled(&window, "크기", AccessibleRole::Button);
    assert_eq!(size_heads.len(), 2);
    size_heads[0].invoke_accessible_default_action();
    size_heads[1].invoke_accessible_default_action();
    labelled(&window, "이름", AccessibleRole::Button)[0].invoke_accessible_default_action();
    assert_eq!(
        sorted.borrow().as_slice(),
        [(0, 1, false), (1, 1, false), (0, 0, false)]
    );
}

/// A window whose left pane lists two files in 자세히 with the default columns (크기, 수정한
/// 날짜, 종류), and what its header's callbacks and a drop check were called with.
fn header_window() -> (MainWindow, PaneModels, Rc<RefCell<Vec<String>>>) {
    let window = window();
    let pane = listed_pane(vec![
        entry("a.txt", EntryKind::File, 10),
        entry("bb.txt", EntryKind::File, 2000),
    ]);
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));
    let calls = Rc::new(RefCell::new(Vec::new()));
    let note = |calls: &Rc<RefCell<Vec<String>>>| {
        let calls = Rc::clone(calls);
        move |call: String| calls.borrow_mut().push(call)
    };
    let record = note(&calls);
    window.on_column_toggled(move |side, column| record(format!("toggle {side} {column}")));
    let record = note(&calls);
    window.on_column_moved(move |side, from, slot| record(format!("move {side} {from} {slot}")));
    let record = note(&calls);
    window.on_column_resized(move |side, column, width| {
        record(format!("width {side} {column} {width}"));
    });
    let record = note(&calls);
    window.on_column_texts(move |side, column, first, count| {
        record(format!("texts {side} {column} {first} {count}"));
        "1023.9 KB\n2 KB".into()
    });
    let record = note(&calls);
    window.on_sort(move |side, column, then| record(format!("sort {side} {column} {then}")));
    let record = note(&calls);
    window.on_drop_check(move |_, _, _, _, _| {
        record("drop-check".to_owned());
        DragAction::None
    });
    (window, models, calls)
}

/// The box of the column head called `title` of the left pane (the right pane, empty, has the
/// name's head too).
fn head_box(window: &MainWindow, title: &str) -> (LogicalPosition, LogicalSize) {
    let found = labelled(window, title, AccessibleRole::Button);
    assert!(!found.is_empty(), "a head {title}");
    (found[0].absolute_position(), found[0].size())
}

#[test]
fn a_head_s_menu_shows_and_hides_the_columns() {
    use slint::platform::PointerEventButton;
    let (window, _models, calls) = header_window();
    for (head, item, call) in [
        ("크기", "만든 날짜", "toggle 0 4"),
        ("이름", "종류", "toggle 0 3"),
        ("종류", "속성", "toggle 0 6"),
    ] {
        labelled(&window, head, AccessibleRole::Button)[0]
            .mock_single_click(PointerEventButton::Right);
        choose_item(&window, item);
        assert_eq!(calls.borrow_mut().drain(..).collect::<Vec<_>>(), [call]);
    }
}

#[test]
fn a_head_dragged_along_the_header_moves_its_column_and_drags_no_items() {
    let (window, _models, calls) = header_window();
    let (size_at, _) = head_box(&window, "크기");
    let (kind_at, kind_size) = head_box(&window, "종류");
    let kind = LogicalPosition::new(
        kind_at.x + kind_size.width / 2.0,
        kind_at.y + kind_size.height / 2.0,
    );
    // Let go before the size's middle: the kind goes first.
    let before_size = LogicalPosition::new(size_at.x + 4.0, kind.y);
    drag_to(&window, kind, before_size);
    pointer(&window, "release", before_size);
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["move 0 2 0"],
        "no click of the head, and no drop asked about"
    );
    // A short move is a click: it sorts.
    pointer(&window, "press", kind);
    pointer(&window, "move", LogicalPosition::new(kind.x + 3.0, kind.y));
    pointer(
        &window,
        "release",
        LogicalPosition::new(kind.x + 3.0, kind.y),
    );
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["sort 0 3 false"]
    );
}

#[test]
fn a_border_dragged_resizes_its_own_column_and_a_double_click_fits_it() {
    let (window, _models, calls) = header_window();
    let head = |title: &str| head_box(&window, title);
    let ((modified_at, modified_size), size, (kind_at, _)) =
        (head("수정한 날짜"), head("크기"), head("종류"));
    // The border before 수정한 날짜, in the gap between it and 크기.
    let border = LogicalPosition::new(
        modified_at.x - 5.0,
        modified_at.y + modified_size.height / 2.0,
    );
    let at = |dx: f32| LogicalPosition::new(border.x + dx, border.y);
    pointer(&window, "move", border);
    pointer(&window, "press", border);
    for step in [5.0, 10.0, 20.0] {
        pointer(&window, "move", at(step));
    }
    // While it is dragged: the column is narrower, the columns after it stay where they are, and
    // those before it go along with the border, the name taking the room.
    let (now_at, now_size) = head("수정한 날짜");
    assert_eq!(
        (now_at.x, now_size.width),
        (modified_at.x + 20.0, modified_size.width - 20.0)
    );
    assert_eq!(head("종류").0.x, kind_at.x);
    assert_eq!(head("크기").0.x, size.0.x + 20.0);
    assert_eq!(head("크기").1, size.1);
    pointer(&window, "release", at(20.0));
    // That column alone takes a new width.
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["width 0 2 106"]
    );

    // To the left, past the room of the name: the name keeps its least room and the list
    // scrolls, so that the columns after the border stay where they are; the column goes as
    // wide as a column can.
    let (name_at, _) = head("이름");
    pointer(&window, "press", border);
    for step in [-100.0, -300.0, -600.0] {
        pointer(&window, "move", at(step));
    }
    assert_eq!(head("수정한 날짜").1.width, 600.0);
    assert_eq!(head("종류").0.x, kind_at.x);
    assert_eq!(
        labelled(&window, "이름", AccessibleRole::Button).len(),
        1,
        "the left pane's name scrolled out of view (the right pane's shows)"
    );
    pointer(&window, "release", at(-600.0));
    assert_eq!(
        calls.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["width 0 2 600"]
    );
    // Rust kept the old width here: the list scrolls back, the heads as they were.
    assert_eq!(head("이름").0.x, name_at.x);

    // A double click fits the column to the texts of the rows on screen.
    for _ in 0..2 {
        pointer(&window, "press", border);
        pointer(&window, "release", border);
    }
    let noted = calls.borrow_mut().drain(..).collect::<Vec<_>>();
    assert_eq!(noted.len(), 2, "{noted:?}");
    assert!(noted[0].starts_with("texts 0 2 0 "), "{noted:?}");
    assert!(noted[1].starts_with("width 0 2 "), "{noted:?}");
}

#[test]
fn every_column_chosen_shows_and_the_list_scrolls_sideways_when_they_overflow() {
    let window = window();
    window.window().set_size(LogicalSize::new(900.0, 560.0));
    let mut pane = listed_pane(vec![
        entry("a.txt", EntryKind::File, 10),
        entry("bb.txt", EntryKind::File, 2000),
    ]);
    pane.active_tab_mut().set_view(FolderView {
        columns: Column::OPTIONAL.into_iter().map(ShownColumn::new).collect(),
        ..FolderView::default()
    });
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));
    let head = |title: &str| head_box(&window, title);

    // The heads are wider than the pane's list: the name keeps its least room, and the last
    // heads are right of the list, out of view.
    let list = labelled(&window, "항목 목록", AccessibleRole::List).remove(0);
    let (list_at, list_size) = (list.absolute_position(), list.size());
    let right = list_at.x + list_size.width;
    let (name_at, name_size) = head("이름");
    assert!(name_size.width >= 120.0 - 26.0, "{name_size:?}");
    let shown = || -> Vec<(Column, LogicalPosition, LogicalSize)> {
        Column::OPTIONAL
            .into_iter()
            .filter_map(|column| {
                let found = labelled(&window, column.title(), AccessibleRole::Button);
                found
                    .first()
                    .map(|head| (column, head.absolute_position(), head.size()))
            })
            .collect()
    };
    assert!(
        !shown()
            .iter()
            .any(|(column, ..)| *column == Column::Attributes),
        "속성 is out of view"
    );

    // A sideways scroll moves the heads and the rows together.
    let name_text = || labelled(&window, "a.txt", AccessibleRole::Text)[0].absolute_position();
    let text_at = name_text();
    let scroll = |dx: f32| {
        window
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position: LogicalPosition::new(list_at.x + 60.0, list_at.y + 40.0),
                delta_x: dx,
                delta_y: 0.0,
            });
        // A wheel scrolls smoothly, for a short while.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
    };
    // (Not so far that the name goes out of view: the right pane's would be the one found.)
    scroll(-40.0);
    let moved = name_at.x - head("이름").0.x;
    assert_eq!(moved, 40.0, "the heads scrolled");
    assert_eq!(text_at.x - name_text().x, moved, "the rows with them");
    // Every column chosen comes into view, as wide as it is, and the list scrolls as far as
    // the last one, whole, and no further.
    let mut seen = Vec::new();
    for _ in 0..12 {
        for (column, _, size) in shown() {
            assert_eq!(
                f64::from(size.width),
                f64::from(column.default_width()),
                "{column:?}"
            );
            if !seen.contains(&column) {
                seen.push(column);
            }
        }
        scroll(-100.0);
    }
    assert_eq!(seen.len(), Column::OPTIONAL.len(), "{seen:?}");
    let (_, last_at, last_size) = shown().pop().expect("a head in view");
    let end = last_at.x + last_size.width;
    assert!(end <= right && end > right - 20.0, "{end} {right}");
}

#[test]
fn the_columns_order_and_widths_come_from_the_menus_and_the_keys() {
    use slint::platform::PointerEventButton;
    let (window, _models, calls) = header_window();
    let taken = || calls.borrow_mut().drain(..).collect::<Vec<_>>();
    // The sub-menus open after the menu they are in: of two items of one title (a column's,
    // to show or hide it and to move it), the last is the sub-menu's (the heads are buttons).
    let choose_last = |title: &str| {
        labelled(&window, title, AccessibleRole::Text)
            .pop()
            .unwrap_or_else(|| panic!("the menu shows {title}"))
            .mock_single_click(PointerEventButton::Left);
    };
    let open_head_menu = |path: &[&str]| {
        labelled(&window, "크기", AccessibleRole::Button)[0]
            .mock_single_click(PointerEventButton::Right);
        for item in path {
            choose_last(item);
        }
    };
    // The head's menu: 열 순서 ▸ a column ▸ 왼쪽으로 or 오른쪽으로 (크기, 수정한 날짜, 종류).
    for (column, item, call) in [
        ("크기", "오른쪽으로", "move 0 0 2"),
        ("종류", "왼쪽으로", "move 0 2 1"),
        ("수정한 날짜", "왼쪽으로", "move 0 1 0"),
    ] {
        open_head_menu(&["열 순서", column]);
        choose_item(&window, item);
        assert_eq!(taken(), [call], "{column} {item}");
    }
    // The first column goes no further left, the last no further right.
    for (column, item) in [("크기", "왼쪽으로"), ("종류", "오른쪽으로")] {
        open_head_menu(&["열 순서", column]);
        choose_item(&window, item);
        assert_eq!(taken(), Vec::<String>::new(), "{column} {item}");
        for _ in 0..3 {
            press(&window, Key::Escape);
        }
    }
    // 모든 열 너비 맞추기 fits each column after the name.
    open_head_menu(&[]);
    choose_item(&window, "모든 열 너비 맞추기");
    let fitted = taken();
    assert_eq!(fitted.len(), 6, "{fitted:?}");
    for (pair, column) in fitted.chunks(2).zip([1, 2, 3]) {
        assert!(
            pair[0].starts_with(&format!("texts 0 {column} ")),
            "{fitted:?}"
        );
        assert!(
            pair[1].starts_with(&format!("width 0 {column} ")),
            "{fitted:?}"
        );
    }
    // The same from Rust (the key) for the pane it names.
    window.invoke_fit_columns(0);
    assert_eq!(taken().len(), 6);

    // The 보기 menu's 열 has them too, for the active pane.
    let record = Rc::clone(&calls);
    window.on_command(move |name| record.borrow_mut().push(format!("command {name}")));
    let button = labelled(&window, "보기", AccessibleRole::Button).remove(0);
    button.invoke_accessible_default_action();
    for item in ["열", "열 순서", "수정한 날짜", "오른쪽으로"] {
        choose_last(item);
    }
    assert_eq!(taken(), ["move 0 1 3"]);
    button.invoke_accessible_default_action();
    choose_item(&window, "열");
    // (The test window has no keys set: the item's title is the name alone.)
    choose_item(&window, "모든 열 너비 맞추기");
    assert_eq!(taken(), ["command fit-columns"]);
}

#[test]
fn the_arrows_scroll_details_sideways() {
    let window = window();
    window.window().set_size(LogicalSize::new(900.0, 560.0));
    let mut pane = listed_pane(vec![entry("a.txt", EntryKind::File, 10)]);
    pane.active_tab_mut().set_view(FolderView {
        columns: Column::OPTIONAL.into_iter().map(ShownColumn::new).collect(),
        ..FolderView::default()
    });
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));
    let size_x = || head_box(&window, "크기").0.x;
    let at = size_x();
    // A step to the right: the heads and the rows go left.
    window.invoke_scroll_sideways(0, true);
    assert_eq!(size_x(), at - 80.0);
    // Back, and no further than the start.
    window.invoke_scroll_sideways(0, false);
    window.invoke_scroll_sideways(0, false);
    assert_eq!(size_x(), at);
    // As far as the last column, whole.
    for _ in 0..20 {
        window.invoke_scroll_sideways(0, true);
    }
    let list = labelled(&window, "항목 목록", AccessibleRole::List).remove(0);
    let right = list.absolute_position().x + list.size().width;
    let (last_at, last_size) = head_box(&window, "속성");
    assert_eq!(last_at.x + last_size.width + 12.0, right);

    // The bar under the rows: a press at its left end scrolls back to the start, a drag of its
    // thumb to the right as far as the end.
    let bar_y = list.absolute_position().y + list.size().height + 5.0;
    let left = list.absolute_position().x;
    pointer(&window, "press", LogicalPosition::new(left + 2.0, bar_y));
    pointer(&window, "release", LogicalPosition::new(left + 2.0, bar_y));
    assert_eq!(size_x(), at);
    pointer(&window, "press", LogicalPosition::new(left + 10.0, bar_y));
    pointer(&window, "move", LogicalPosition::new(left + 60.0, bar_y));
    pointer(&window, "move", LogicalPosition::new(right, bar_y));
    pointer(&window, "release", LogicalPosition::new(right, bar_y));
    let (last_at, last_size) = head_box(&window, "속성");
    assert_eq!(last_at.x + last_size.width + 12.0, right);
}

#[test]
fn a_right_click_beside_the_heads_opens_their_menu_with_its_marks() {
    use slint::platform::PointerEventButton;
    let (window, _models, _calls) = header_window();
    // Left of the name's head, in the header's padding.
    let (name_at, name_size) = head_box(&window, "이름");
    let beside = LogicalPosition::new(name_at.x - 6.0, name_at.y + name_size.height / 2.0);
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position: beside,
        button: PointerEventButton::Right,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: beside,
            button: PointerEventButton::Right,
        });
    // The marks of the columns that show: 크기, 수정한 날짜, 종류.
    assert_eq!(
        ElementHandle::find_by_accessible_label(&window, "✓").count(),
        3
    );
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
fn a_toast_stays_while_the_pointer_is_over_it() {
    let window = window();
    let shown = |text: &str| !labelled(&window, text, AccessibleRole::Text).is_empty();
    let pointer = |position: LogicalPosition| {
        window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        // Runs the toast's `changed` handlers.
        slint::platform::update_timers_and_animations();
    };
    window.invoke_show_toast("항목 1개를 복사했습니다".into(), "되돌리기".into(), 3);
    slint::platform::update_timers_and_animations();
    let toast = labelled(&window, "항목 1개를 복사했습니다", AccessibleRole::Text)
        .into_iter()
        .next()
        .expect("the toast shows");
    let (at, size) = (toast.absolute_position(), toast.size());
    let over = LogicalPosition::new(at.x + 8.0, at.y + size.height / 2.0);

    // Over it, its time does not run out.
    pointer(over);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(20));
    assert!(shown("항목 1개를 복사했습니다"));
    // Once the pointer leaves, it has its 8 seconds again in full.
    pointer(LogicalPosition::new(10.0, 10.0));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(7));
    assert!(shown("항목 1개를 복사했습니다"));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1500));
    assert!(!shown("항목 1개를 복사했습니다"));
}

#[test]
fn a_busy_notice_stays_until_its_work_is_done() {
    let window = window();
    let shown = |text: &str| !labelled(&window, text, AccessibleRole::Text).is_empty();
    let pointer = |position: LogicalPosition| {
        window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        slint::platform::update_timers_and_animations();
    };
    window.invoke_show_busy("원본을 확인하는 중…".into(), 7);
    slint::platform::update_timers_and_animations();
    let note = labelled(&window, "원본을 확인하는 중…", AccessibleRole::Text)
        .into_iter()
        .next()
        .expect("the notice shows");
    let (at, size) = (note.absolute_position(), note.size());

    // However long the worker takes, and after the pointer has been over it.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(20));
    assert!(shown("원본을 확인하는 중…"));
    pointer(LogicalPosition::new(at.x + 8.0, at.y + size.height / 2.0));
    pointer(LogicalPosition::new(10.0, 10.0));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(20));
    assert!(shown("원본을 확인하는 중…"));
    // The end of another work leaves it; the end of its own takes it away.
    window.invoke_end_busy(6);
    assert!(shown("원본을 확인하는 중…"));
    window.invoke_end_busy(7);
    assert!(!shown("원본을 확인하는 중…"));

    // A toast that took its place stays when its work ends, and goes in its own time.
    window.invoke_show_busy("원본을 확인하는 중…".into(), 8);
    window.invoke_show_toast("항목 1개를 처리하지 못했습니다".into(), "자세히".into(), 9);
    window.invoke_end_busy(8);
    // Starts the toast's timer, as the event loop would.
    slint::platform::update_timers_and_animations();
    assert!(shown("항목 1개를 처리하지 못했습니다"));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(9));
    assert!(!shown("항목 1개를 처리하지 못했습니다"));
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
    window.on_key(move |text, _, _, _, _| {
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
        ..ConfirmView::default()
    });
    window.set_dialog(DialogKind::Confirm);
    let message =
        "휴지통을 쓸 수 없는 위치입니다.\n항목 1개를 영구 삭제할까요? 되돌릴 수 없습니다.";
    assert_eq!(labelled(&window, message, AccessibleRole::Text).len(), 1);
    // The panes get no key while it shows, and Enter does not delete: it is on 취소.
    let down = char::from(Key::DownArrow).to_string();
    let enter = char::from(Key::Return).to_string();
    chord(&window, &[&down]);
    assert_eq!(keys.borrow().as_slice(), Vec::<String>::new());
    assert_eq!(answers.borrow().as_slice(), Vec::<&str>::new());
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().as_slice(), ["cancelled"]);
    // Esc is No.
    chord(&window, &[&char::from(Key::Escape).to_string()]);
    assert_eq!(answers.borrow().as_slice(), ["cancelled", "cancelled"]);
    // The button deletes.
    let delete: Vec<ElementHandle> = labelled(&window, "영구 삭제", AccessibleRole::Button);
    delete[0].invoke_accessible_default_action();
    assert_eq!(
        answers.borrow().as_slice(),
        ["cancelled", "cancelled", "accepted"]
    );

    // Closed, the panes have the keys again.
    window.set_dialog(DialogKind::None);
    chord(&window, &[&down]);
    assert_eq!(keys.borrow().as_slice(), [down]);
}

#[test]
fn a_confirm_is_answered_from_the_keyboard_on_the_button_it_is_on() {
    let window = window();
    let answers = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&answers);
    window.on_confirm_accepted(move || record.borrow_mut().push("accepted"));
    let record = Rc::clone(&answers);
    window.on_dialog_cancelled(move || record.borrow_mut().push("cancelled"));
    let show = |danger: bool, count: usize| {
        window.set_dialog(DialogKind::None);
        window.set_confirm_view(ConfirmView {
            title: "영구 삭제".into(),
            message: format!("항목 {count}개를 영구 삭제할까요? 되돌릴 수 없습니다.").into(),
            ok_text: "영구 삭제".into(),
            danger,
            ..ConfirmView::default()
        });
        window.set_dialog(DialogKind::Confirm);
        // Runs the dialog's `changed` handlers.
        slint::platform::update_timers_and_animations();
    };
    let key = |key: Key| char::from(key).to_string();
    let (tab, back, space, enter) = (
        key(Key::Tab),
        key(Key::Backtab),
        " ".to_owned(),
        key(Key::Return),
    );

    // A danger confirm starts on 취소: Space and Enter answer No there.
    show(true, 1);
    chord(&window, &[&space]);
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().as_slice(), ["cancelled", "cancelled"]);
    // Tab goes to the delete button, and Space or Enter there deletes.
    chord(&window, &[&tab]);
    chord(&window, &[&space]);
    assert_eq!(answers.borrow().last(), Some(&"accepted"));
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().len(), 4);
    assert_eq!(answers.borrow().last(), Some(&"accepted"));
    // Tab past the last button and Shift+Tab come round to 취소.
    chord(&window, &[&tab]);
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().last(), Some(&"cancelled"));
    chord(&window, &[&back]);
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().last(), Some(&"accepted"));
    chord(&window, &[&back]);
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().last(), Some(&"cancelled"));

    // A new confirm starts on 취소 again.
    chord(&window, &[&tab]);
    answers.borrow_mut().clear();
    show(true, 2);
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().as_slice(), ["cancelled"]);

    // Any other confirm is on no button: Enter is Yes, Space nothing, until Tab picks one.
    answers.borrow_mut().clear();
    show(false, 3);
    chord(&window, &[&space]);
    assert_eq!(answers.borrow().as_slice(), Vec::<&str>::new());
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().as_slice(), ["accepted"]);
    chord(&window, &[&tab]);
    chord(&window, &[&enter]);
    assert_eq!(answers.borrow().as_slice(), ["accepted", "cancelled"]);
}

#[test]
fn the_copies_an_undo_could_not_trash_are_listed_by_their_whole_paths() {
    let window = window();
    let in_backup = |name: &str| {
        VPath::local(std::env::temp_dir().join("백업").join(name)).expect("a local path")
    };
    let copies = vec![in_backup("a.txt"), in_backup("docs")];
    let record = rf_jobs::UndoRecord::Copied {
        created: Vec::new(),
        overwritten: 0,
        at: None,
    };
    let originals = vec![in_backup("원본 a.txt"), in_backup("원본 docs")];
    let confirm = rf_ui::dialogs::Confirm::delete_copies(copies.clone(), originals, record);
    window.set_confirm_view(ui_bridge::confirm_view(&confirm));
    window.set_dialog(DialogKind::Confirm);
    for copy in &copies {
        assert_eq!(
            labelled(&window, &copy.to_string(), AccessibleRole::Text).len(),
            1,
            "{copy}"
        );
    }
    for text in [
        "복사본을 휴지통으로 보낼 수 없습니다. 원본은 그대로 있습니다.",
        "복사본 2개를 영구 삭제할까요? 되돌릴 수 없습니다.",
    ] {
        assert_eq!(
            labelled(&window, text, AccessibleRole::Text).len(),
            1,
            "{text}"
        );
    }
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
        // The one on top: the dialog's, over the panes' lists.
        .rev()
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
        // The one on top: the dialog's, over the panes' lists.
        .rev()
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
    let chips: Vec<SharedString> = vec!["설계안.pdf".into(), "assets".into()];
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
    window.on_key(move |text, _, _, _, _| {
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

    // The 보기 button opens the menu whose 테마 menu chooses a theme.
    let button = labelled(&window, "보기", AccessibleRole::Button);
    assert_eq!(button.len(), 1);
    let click = |label: &str| {
        ElementHandle::find_by_accessible_label(&window, label)
            .next()
            .unwrap_or_else(|| panic!("the menu shows {label}"))
            .mock_single_click(slint::platform::PointerEventButton::Left);
    };
    button[0].invoke_accessible_default_action();
    click("테마");
    // The menu is a popup window of its own: the element clicks there.
    click("어둡게");
    assert_eq!(chosen.borrow().as_slice(), [2]);

    // The marks of the menus: the theme chosen, and 숨김 파일 보기 as `show-hidden` says (with
    // the marks of the layout's 폴더 트리 and 듀얼 패널, which are on).
    window.set_show_tree(false);
    window.set_dual_pane(false);
    window.set_show_extensions(false);
    let commands = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&commands);
    window.on_command(move |name| record.borrow_mut().push(name.to_string()));
    let open_themes = || {
        button[0].invoke_accessible_default_action();
        click("테마");
        ElementHandle::find_by_accessible_label(&window, "✓").count()
    };
    assert_eq!(open_themes(), 1);
    // Choosing the theme that is chosen keeps its mark.
    click("어둡게");
    assert_eq!(chosen.borrow().as_slice(), [2, 2]);
    assert_eq!(open_themes(), 1);
    click("어둡게");
    window.set_show_hidden(true);
    assert_eq!(open_themes(), 2);
    click("어둡게");
    button[0].invoke_accessible_default_action();
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

/// The left pane of [`drag_window`] shown in `mode`.
fn left_in(window: &MainWindow, models: &[PaneModels; 2], mode: ViewMode) {
    let mut left = listed_pane(vec![
        entry("docs", EntryKind::Dir, 0),
        entry("x.txt", EntryKind::File, 10),
    ]);
    left.active_tab_mut().set_view(FolderView {
        mode,
        ..FolderView::default()
    });
    window.set_left_pane(models[0].view(&left, true));
}

#[test]
fn icons_and_the_list_take_presses_drags_and_menus_as_rows_do() {
    use slint::platform::PointerEventButton;
    let (window, calls, models, _tree) = drag_window();
    let menus = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&menus);
    window.on_row_menu(move |side, i| record.borrow_mut().push((side, i)));
    let opened = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&opened);
    window.on_row_activated(move |side, i| record.borrow_mut().push((side, i)));

    for mode in [
        ViewMode::LargeIcons,
        ViewMode::MediumIcons,
        ViewMode::SmallIcons,
        ViewMode::List,
    ] {
        left_in(&window, &models, mode);
        // One item each, as list items named for screen readers, and no rows of 자세히.
        let x_txt = centre_of(&window, "x.txt", AccessibleRole::ListItem);
        let backup = centre_of(&window, "백업", AccessibleRole::ListItem);
        // The keys know the layout: a row of icons holds several, a column of 목록 several.
        assert!(window.get_line_items() > 1, "{mode:?}");
        assert!(window.get_page_rows() >= 1, "{mode:?}");

        // A click presses and releases the item; a double click opens it.
        pointer(&window, "move", x_txt);
        pointer(&window, "press", x_txt);
        pointer(&window, "release", x_txt);
        assert_eq!(
            calls.rows.borrow_mut().drain(..).collect::<Vec<_>>(),
            [("press", 0, 1), ("release", 0, 1)],
            "{mode:?}"
        );
        // Two clicks at once: the testing backend's time stands still.
        for _ in 0..2 {
            pointer(&window, "press", x_txt);
            pointer(&window, "release", x_txt);
        }
        assert_eq!(
            opened.borrow_mut().drain(..).next_back(),
            Some((0, 1)),
            "{mode:?}"
        );
        calls.rows.borrow_mut().clear();
        let item = labelled(&window, "x.txt", AccessibleRole::ListItem).remove(0);
        // A right click asks for the item's menu first.
        item.mock_single_click(PointerEventButton::Right);
        assert_eq!(menus.borrow_mut().drain(..).collect::<Vec<_>>(), [(0, 1)]);
        press(&window, Key::Escape);

        // A drag onto the other pane's folder drops there, with the payload Rust made.
        drag_to(&window, x_txt, backup);
        pointer(&window, "release", backup);
        assert_eq!(
            calls.drops.borrow_mut().drain(..).collect::<Vec<_>>(),
            [(42, DropPlace::List, 1, 0, DragAction::Move)],
            "{mode:?}"
        );
        // And an item of the other pane onto this pane's folder item asks this list about it.
        let docs = centre_of(&window, "docs", AccessibleRole::ListItem);
        let y_txt = centre_of(&window, "y.txt", AccessibleRole::ListItem);
        drag_to(&window, y_txt, docs);
        assert_eq!(
            calls.checks.borrow().last().copied(),
            Some((42, DropPlace::List, 0, 0, DragAction::Move)),
            "{mode:?}"
        );
        pointer(&window, "release", docs);
        calls.rows.borrow_mut().clear();
        calls.drops.borrow_mut().clear();
    }
}

#[test]
fn a_long_folder_makes_only_the_items_on_screen_and_scrolls_to_the_cursor() {
    let window = window();
    let pressed = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&pressed);
    window.on_row_clicked(move |_, i, _, _| record.borrow_mut().push(i));
    let mut pane = listed_pane(
        (0..100_000)
            .map(|i| entry(&format!("f{i:05}"), EntryKind::File, 1))
            .collect(),
    );
    let models = PaneModels::default();
    for mode in [ViewMode::MediumIcons, ViewMode::List] {
        pane.active_tab_mut().set_view(FolderView {
            mode,
            ..FolderView::default()
        });
        window.set_left_pane(models.view(&pane, true));
        window.invoke_scroll_to(0, 0);
        // Only the items on screen exist, in either view.
        assert_eq!(
            labelled(&window, "f00001", AccessibleRole::ListItem).len(),
            1
        );
        assert_eq!(
            labelled(&window, "f50000", AccessibleRole::ListItem).len(),
            0,
            "{mode:?}"
        );
        // Scrolled to an item far away, it is made, and takes a press.
        window.invoke_scroll_to(0, 50_000);
        let far = labelled(&window, "f50000", AccessibleRole::ListItem);
        assert_eq!(far.len(), 1, "{mode:?}");
        assert_eq!(
            labelled(&window, "f00001", AccessibleRole::ListItem).len(),
            0
        );
        // Near its top: in the testing backend the bottom of the list is under the status bar.
        let at = far[0].absolute_position();
        pointer(
            &window,
            "press",
            LogicalPosition::new(at.x + 8.0, at.y + 8.0),
        );
        pointer(
            &window,
            "release",
            LogicalPosition::new(at.x + 8.0, at.y + 8.0),
        );
        assert_eq!(
            pressed.borrow_mut().drain(..).collect::<Vec<_>>(),
            [50_000],
            "{mode:?}"
        );
    }
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
fn a_refused_place_says_why_while_the_drag_stays_over_it() {
    let (window, _calls, _models, _tree) = drag_window();
    // As the controller does: every check tells the window whether the place refuses, and why.
    let weak = window.as_weak();
    window.on_drop_check(move |_, place, side, _, action| {
        let refused = side == 0 && place == DropPlace::List;
        if let Some(window) = weak.upgrade() {
            let why = if refused {
                "여기에 놓을 수 없습니다: 같은 폴더"
            } else {
                ""
            };
            window.invoke_drop_refused(place, side, why.into());
        }
        if refused { DragAction::None } else { action }
    });
    let shown = || {
        !labelled(
            &window,
            "여기에 놓을 수 없습니다: 같은 폴더",
            AccessibleRole::Text,
        )
        .is_empty()
    };
    let x_txt = centre_of(&window, "x.txt", AccessibleRole::ListItem);
    let docs = centre_of(&window, "docs", AccessibleRole::ListItem);
    let backup = centre_of(&window, "백업", AccessibleRole::ListItem);
    // The toolbar takes no drop.
    let toolbar = LogicalPosition::new(docs.x, 20.0);

    // Over the pane the items are in: the reason shows, and stays while the pointer rests there.
    drag_to(&window, x_txt, docs);
    // Runs what the event loop runs between the moves (timers start).
    slint::platform::update_timers_and_animations();
    assert!(shown());
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(5));
    assert!(shown(), "still over the place that refuses");
    // Off to a place that takes no drop, it goes; back over the place, it shows again.
    pointer(&window, "move", toolbar);
    assert!(!shown());
    pointer(&window, "move", docs);
    assert!(shown());
    // A place that takes the drag says nothing.
    pointer(&window, "move", backup);
    assert!(!shown());
    // The end of the drag takes it away.
    pointer(&window, "move", docs);
    assert!(shown());
    pointer(&window, "release", docs);
    assert!(!shown());
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

#[test]
fn panes_stay_as_wide_as_each_other_whatever_their_tabs() {
    let window = window();
    let tabs = |count: usize| {
        let tabs: Vec<rf_ui::TabItem> = (0..count)
            .map(|i| rf_ui::TabItem {
                title: format!("폴더 {i}").into(),
                active: i + 1 == count,
                ..rf_ui::TabItem::default()
            })
            .collect();
        ModelRc::new(slint::VecModel::from(tabs))
    };
    let width = |label: &str| {
        labelled(&window, label, AccessibleRole::Groupbox)
            .first()
            .map_or_else(|| panic!("the {label}"), |pane| pane.size().width)
    };
    for (left, right) in [(1, 1), (3, 2), (12, 1)] {
        window.set_left_pane(PaneView {
            tabs: tabs(left),
            active_tab: i32::try_from(left - 1).expect("a few tabs"),
            ..PaneView::default()
        });
        window.set_right_pane(PaneView {
            tabs: tabs(right),
            active_tab: i32::try_from(right - 1).expect("a few tabs"),
            ..PaneView::default()
        });
        // The strip follows the tab that shows once the change has been taken in.
        slint::platform::update_timers_and_animations();
        let (left_width, right_width) = (width("왼쪽 패널"), width("오른쪽 패널"));
        assert!(
            (left_width - right_width).abs() < 1.0,
            "{left} and {right} tabs: {left_width} and {right_width}"
        );
    }
    // The last of twelve tabs, which shows, is in view in its scrolled strip.
    let shown = labelled(&window, "폴더 11", AccessibleRole::Tab);
    let pane = labelled(&window, "왼쪽 패널", AccessibleRole::Groupbox).remove(0);
    let (tab_x, pane_x) = (shown[0].absolute_position().x, pane.absolute_position().x);
    assert!(
        tab_x >= pane_x && tab_x + shown[0].size().width <= pane_x + pane.size().width,
        "tab at {tab_x}, pane at {pane_x}"
    );
}

/// A window whose left pane has the tabs `기획`, `디자인` and `회의록`, the last one pinned and
/// the first one showing, and whose tab callbacks are noted as lines: `select`, `close` and
/// the menu's commands with the pane and the tab, the drag's `check` and `drop` with the tab's
/// id, the pane and the place in the strip (each taken).
fn tab_window() -> (MainWindow, Calls) {
    tab_window_of(&["기획", "디자인", "회의록"], Some(2), 0)
}

/// A window whose left pane has a tab at each folder of `names` (under the temporary folder),
/// tab `pinned` pinned and tab `shown` showing, its strip scrolled to show it; its tab
/// callbacks are noted as [`tab_window`] notes them.
fn tab_window_of(names: &[&str], pinned: Option<usize>, shown: usize) -> (MainWindow, Calls) {
    let window = window();
    let at = |name: &str| VPath::local(std::env::temp_dir().join(name)).expect("a local path");
    let (mut pane, _) = Pane::new(at(names[0]), VisibilityFilter::default());
    for name in &names[1..] {
        let _ = pane.add_tab(at(name));
    }
    if let Some(pinned) = pinned {
        pane.tabs_mut()[pinned].set_pinned(true);
    }
    pane.select_tab(shown);
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));
    let calls = Calls::default();
    let record = calls.clone();
    window.on_tab_selected(move |side, i| record.note(format!("select {side} {i}")));
    let record = calls.clone();
    window.on_tab_closed(move |side, i| record.note(format!("close {side} {i}")));
    let record = calls.clone();
    window.on_tab_command(move |side, i, name| record.note(format!("{name} {side} {i}")));
    let record = calls.clone();
    window.on_tab_move_check(move |data, side, slot| {
        let id = ui_bridge::tab_id(&data).unwrap_or(0);
        record.note(format!("check {id} {side} {slot}"));
        rf_ui::TabDrop::Moves
    });
    let record = calls.clone();
    window.on_tab_move_dropped(move |data, side, slot| {
        let id = ui_bridge::tab_id(&data).unwrap_or(0);
        record.note(format!("drop {id} {side} {slot}"));
        true
    });
    slint::platform::update_timers_and_animations();
    (window, calls)
}

/// As the controller does: a payload of no tab from the start (a drag begins only on a press
/// whose area has one), and the pressed tab's (id 5) once the press has shown it.
fn ready_tab_drags(window: &MainWindow, calls: &Calls) {
    window.set_tab_drag_data(ui_bridge::tab_payload(0));
    let weak = window.as_weak();
    let record = calls.clone();
    window.on_tab_selected(move |side, i| {
        record.note(format!("select {side} {i}"));
        if let Some(window) = weak.upgrade() {
            window.set_tab_drag_data(ui_bridge::tab_payload(5));
        }
    });
}

fn the_tab(window: &MainWindow, title: &str) -> ElementHandle {
    let mut found = labelled(window, title, AccessibleRole::Tab);
    assert_eq!(found.len(), 1, "one tab {title}");
    found.remove(0)
}

/// Clicks the item `title` of the menu that shows.
fn choose_item(window: &MainWindow, title: &str) {
    ElementHandle::find_by_accessible_label(window, title)
        .next()
        .unwrap_or_else(|| panic!("the menu shows {title}"))
        .mock_single_click(slint::platform::PointerEventButton::Left);
}

#[test]
fn a_tab_s_menu_closes_copies_pins_and_moves_tabs() {
    use slint::platform::PointerEventButton;
    let (window, calls) = tab_window();
    let menu_of = |title: &str| {
        the_tab(&window, title).mock_single_click(PointerEventButton::Right);
    };
    for (item, call) in [
        ("닫기", "close 0 1"),
        ("다른 탭 닫기", "close-others 0 1"),
        ("복제", "duplicate 0 1"),
        ("고정", "pin 0 1"),
        ("반대 패널로 옮기기", "move-other 0 1"),
    ] {
        // A right click opens the menu; it does not show the tab.
        menu_of("디자인");
        assert_eq!(calls.take(), Vec::<String>::new());
        choose_item(&window, item);
        assert_eq!(calls.take(), [call], "{item}");
    }
    // What does not apply is off: no tab to the right of it that is not pinned, no tab closed.
    for item in ["오른쪽 탭 닫기", "닫은 탭 다시 열기"] {
        menu_of("디자인");
        choose_item(&window, item);
        assert_eq!(calls.take(), Vec::<String>::new(), "{item}");
        press(&window, Key::Escape);
    }
    // The pinned tab's menu unpins it.
    menu_of("회의록");
    choose_item(&window, "고정 풀기");
    assert_eq!(calls.take(), ["pin 0 2"]);

    // The pinned tab has no close button, and a middle click does not close it.
    assert_eq!(
        labelled(&window, "탭 닫기", AccessibleRole::Button).len(),
        2
    );
    assert_eq!(
        the_tab(&window, "회의록")
            .accessible_description()
            .as_deref(),
        Some("고정된 탭")
    );
    the_tab(&window, "회의록").mock_single_click(PointerEventButton::Middle);
    assert_eq!(calls.take(), Vec::<String>::new());
    the_tab(&window, "기획").mock_single_click(PointerEventButton::Middle);
    assert_eq!(calls.take(), ["close 0 0"]);
    // A left press shows the tab.
    the_tab(&window, "디자인").mock_single_click(PointerEventButton::Left);
    assert_eq!(calls.take(), ["select 0 1"]);
}

/// Twelve tabs, `폴더 0` to `폴더 11`, more than the strip shows, tab `shown` showing.
fn full_strip_window(shown: usize) -> (MainWindow, Calls) {
    let names: Vec<String> = (0..12).map(|i| format!("폴더 {i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (window, calls) = tab_window_of(&names, None, shown);
    ready_tab_drags(&window, &calls);
    (window, calls)
}

/// The places Rust was asked about, in order, by the strip of the left pane for tab 5.
fn slots_checked(noted: &[String]) -> Vec<i32> {
    noted
        .iter()
        .filter_map(|call| call.strip_prefix("check 5 0 "))
        .map(|slot| slot.parse().expect("a number"))
        .collect()
}

#[test]
fn a_quick_drag_in_a_full_strip_moves_the_tab_not_the_strip() {
    // The strip shows its first tabs, and could scroll on to the others.
    let (window, calls) = full_strip_window(0);
    let first_x = the_tab(&window, "폴더 0").absolute_position().x;
    let from = centre(&the_tab(&window, "폴더 2"));
    let one = the_tab(&window, "폴더 1");
    let (at, size) = (one.absolute_position(), one.size());
    // The left part of the tab before it: the place before that one.
    let to = LogicalPosition::new(at.x + size.width * 0.25, from.y);

    // A quick drag: a press, then big steps sideways, and a release.
    pointer(&window, "press", from);
    pointer(&window, "move", LogicalPosition::new(from.x - 40.0, from.y));
    pointer(&window, "move", to);
    pointer(&window, "release", to);

    let noted = calls.take();
    assert_eq!(noted.first().map(String::as_str), Some("select 0 2"));
    assert_eq!(
        noted.last().map(String::as_str),
        Some("drop 5 0 1"),
        "{noted:?}"
    );
    let still = the_tab(&window, "폴더 0").absolute_position().x;
    assert!(
        (still - first_x).abs() < 0.5,
        "the strip stays: {first_x} {still}"
    );
}

#[test]
fn a_tab_held_at_an_end_of_a_full_strip_scrolls_it_that_way() {
    // The strip shows its last tabs.
    let (window, calls) = full_strip_window(11);
    let pane = labelled(&window, "왼쪽 패널", AccessibleRole::Groupbox).remove(0);
    let from = centre(&the_tab(&window, "폴더 11"));
    // Just inside the strip's left end.
    let left_end = LogicalPosition::new(pane.absolute_position().x + 14.0, from.y);
    let tick = || {
        for _ in 0..30 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(40));
            slint::platform::update_timers_and_animations();
        }
    };

    drag_to(&window, from, left_end);
    let at_first = *slots_checked(&calls.take()).last().expect("asked");
    tick();

    // The tabs before came into view under the pointer, and Rust was asked about each place.
    let slots = slots_checked(&calls.take());
    let last = *slots.last().expect("asked again as the strip moved");
    assert!(last + 2 <= at_first, "{at_first} → {slots:?}");
    // Let go, the strip stops.
    pointer(&window, "release", left_end);
    calls.take();
    tick();
    assert_eq!(slots_checked(&calls.take()), Vec::<i32>::new());
}

#[test]
fn a_tab_dragged_along_its_strip_or_into_the_other_one_asks_rust_where_it_goes() {
    let (window, calls) = tab_window();
    ready_tab_drags(&window, &calls);
    let first = centre(&the_tab(&window, "기획"));
    let last = the_tab(&window, "회의록");
    let (at, size) = (last.absolute_position(), last.size());
    // The right half of the last tab: the place after it.
    let after_last = LogicalPosition::new(at.x + size.width * 0.75, at.y + size.height / 2.0);
    let before_last = LogicalPosition::new(at.x + size.width * 0.25, after_last.y);

    drag_to(&window, first, after_last);
    let noted = calls.take();
    assert_eq!(noted.first().map(String::as_str), Some("select 0 0"));
    assert_eq!(noted.last().map(String::as_str), Some("check 5 0 3"));
    pointer(&window, "move", before_last);
    assert_eq!(calls.take().last().map(String::as_str), Some("check 5 0 2"));
    // Let go over a tab, whose own place takes dragged rows: the tab goes where the strip
    // said, all the same.
    pointer(&window, "move", after_last);
    calls.take();
    pointer(&window, "release", after_last);
    assert_eq!(calls.take(), ["drop 5 0 3"]);
    drag_to(&window, first, before_last);
    calls.take();
    // The other pane's strip (it has no tabs here: its only place is 0).
    let right_new_tab = labelled(&window, "새 탭", AccessibleRole::Button)
        .into_iter()
        .max_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x))
        .expect("the right pane's 새 탭");
    let strip = centre(&right_new_tab);
    pointer(
        &window,
        "move",
        LogicalPosition::new(strip.x + 80.0, strip.y),
    );
    assert_eq!(calls.take().last().map(String::as_str), Some("check 5 1 0"));
    pointer(
        &window,
        "release",
        LogicalPosition::new(strip.x + 80.0, strip.y),
    );
    assert_eq!(calls.take().last().map(String::as_str), Some("drop 5 1 0"));
}

#[test]
fn the_view_menu_s_tab_items_act_on_the_tab_that_shows() {
    let (window, calls) = tab_window();
    let button = labelled(&window, "보기", AccessibleRole::Button).remove(0);
    let open_tabs = || {
        button.invoke_accessible_default_action();
        choose_item(&window, "탭");
    };
    for (item, call) in [
        ("탭 복제", "duplicate 0 0"),
        ("탭 고정", "pin 0 0"),
        ("탭을 반대 패널로 옮기기", "move-other 0 0"),
        ("탭을 오른쪽으로", "move-right 0 0"),
        ("다른 탭 닫기", "close-others 0 0"),
        ("오른쪽 탭 닫기", "close-right 0 0"),
    ] {
        open_tabs();
        choose_item(&window, item);
        assert_eq!(calls.take(), [call], "{item}");
    }
    // The first tab cannot go further left, and no closed tab is there to reopen.
    for item in ["탭을 왼쪽으로", "닫은 탭 다시 열기"] {
        open_tabs();
        choose_item(&window, item);
        assert_eq!(calls.take(), Vec::<String>::new(), "{item}");
        press(&window, Key::Escape);
        press(&window, Key::Escape);
    }
}

#[test]
fn the_filter_box_hands_over_what_is_typed_and_esc_clears_it() {
    let window = window();
    let keys = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&keys);
    window.on_key(move |text, _, _, _, _| {
        record.borrow_mut().push(text.to_string());
        true
    });
    let edits = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&edits);
    window.on_filter_edited(move |text| record.borrow_mut().push(text.to_string()));
    let field = || labelled(&window, "이 폴더에서 거르기", AccessibleRole::TextInput).remove(0);
    field().mock_single_click(slint::platform::PointerEventButton::Left);
    chord(&window, &["a"]);
    chord(&window, &["b"]);
    assert_eq!(edits.borrow().as_slice(), ["a", "ab"]);
    // The keys it takes are not shortcuts of the panes meanwhile.
    chord(&window, &[&char::from(Key::Delete).to_string()]);
    assert_eq!(keys.borrow().as_slice(), Vec::<String>::new());
    // Esc empties it, and the panes have the keys again.
    chord(&window, &[&char::from(Key::Escape).to_string()]);
    assert_eq!(edits.borrow().last().map(String::as_str), Some(""));
    assert_eq!(field().accessible_value().as_deref(), Some(""));
    chord(&window, &["x"]);
    assert_eq!(keys.borrow().as_slice(), ["x"]);
    // It shows the active pane's filter when that is another.
    window.invoke_show_filter("보고".into());
    assert_eq!(field().accessible_value().as_deref(), Some("보고"));
}

#[test]
fn enter_while_a_syllable_is_composed_does_not_take_the_path() {
    let window = window();
    let entered = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&entered);
    window.on_path_entered(move |side, text| record.borrow_mut().push((side, text.to_string())));
    let mut view = window.get_left_pane();
    view.path_text = "/start".into();
    window.set_left_pane(view);
    window.invoke_edit_path(0);
    let enter = char::from(Key::Return).to_string();
    compose(&window, "ㅅ");
    chord(&window, &[&enter]);
    assert_eq!(entered.borrow().as_slice(), Vec::<(i32, String)>::new());
    commit_composition(&window, "사");
    chord(&window, &[&enter]);
    assert_eq!(entered.borrow().as_slice(), [(0, "사".to_owned())]);
}

/// The text of a special key.
fn special(key: Key) -> String {
    char::from(key).to_string()
}

/// Presses `key` alone, then with Shift (Shift+Tab arrives as Tab with Shift held).
fn press(window: &MainWindow, key: Key) {
    chord(window, &[&special(key)]);
}

fn press_shifted(window: &MainWindow, key: Key) {
    chord(window, &[&special(Key::Shift), &special(key)]);
}

/// Records each call of a dialog's callbacks as a line, in order.
#[derive(Clone, Default)]
struct Calls(Rc<RefCell<Vec<String>>>);

impl Calls {
    fn note(&self, call: impl Into<String>) {
        self.0.borrow_mut().push(call.into());
    }

    fn take(&self) -> Vec<String> {
        self.0.borrow_mut().drain(..).collect()
    }
}

#[test]
fn the_toolbar_and_the_tabs_say_the_keys_of_their_commands() {
    let window = window();
    let description = |label: &str| {
        labelled(&window, label, AccessibleRole::Button)[0]
            .accessible_description()
            .map(|text| text.to_string())
    };
    let cases = [
        ("뒤로", "Alt+←", "⌘["),
        ("위로", "Alt+↑", "⌘↑"),
        ("새 폴더", "Ctrl+Shift+N", "⇧⌘N"),
        ("복사…", "F7", "F7"),
        ("이동…", "F8", "F8"),
        ("삭제", "Delete", "⌘⌫"),
        ("이름 바꾸기", "F2", "↩"),
        ("되돌리기", "Ctrl+Z", "⌘Z"),
        ("보기", "F10", "⌘J"),
        ("새 탭", "Ctrl+T", "⌘T"),
    ];
    for (os, column) in [(Os::Windows, 1), (Os::MacOs, 2), (Os::Other, 1)] {
        ui_bridge::show_shortcuts(&window, os);
        // The tab menus' keys.
        let shown = window.global::<rf_ui::Shortcuts<'_>>();
        let tab_keys = [
            shown.get_reopen_tab(),
            shown.get_move_tab_left(),
            shown.get_move_tab_right(),
        ];
        let wanted = if column == 1 {
            ["Ctrl+Shift+T", "Ctrl+Shift+PageUp", "Ctrl+Shift+PageDown"]
        } else {
            ["⇧⌘T", "⇧⌘←", "⇧⌘→"]
        };
        assert_eq!(tab_keys.map(|key| key.to_string()), wanted, "{os:?}");
        for case in cases {
            let keys = if column == 1 { case.1 } else { case.2 };
            assert_eq!(
                description(case.0).as_deref(),
                Some(keys),
                "{os:?} {}",
                case.0
            );
        }
    }
}

#[test]
fn the_shortcuts_reach_the_filter_box_and_the_view_menu() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_filter_edited(move |text| record.note(format!("filter {text}")));
    let record = calls.clone();
    window.on_theme_chosen(move |choice| record.note(format!("theme {choice}")));
    let record = calls.clone();
    window.on_key(move |text, _, _, _, _| {
        record.note(format!("key {text}"));
        false
    });

    // The filter takes the keys with what it held selected: what is typed replaces it.
    window.invoke_show_filter("old".into());
    window.invoke_focus_filter();
    chord(&window, &["x"]);
    assert_eq!(calls.take(), ["filter x"]);
    press(&window, Key::Escape);
    assert_eq!(calls.take(), ["filter "]);

    // The 보기 menu opens with the keys on it: after the 보기 방식, 정렬 and 열 menus, ↓ goes to
    // the 테마 menu, → opens it, ↓ goes to its first theme, 밝게, and Enter takes it.
    window.invoke_show_view_menu();
    for _ in 0..4 {
        press(&window, Key::DownArrow);
    }
    press(&window, Key::RightArrow);
    press(&window, Key::DownArrow);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["theme 1"]);
    // Closed, the keys are the panes' again.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    chord(&window, &["a"]);
    assert_eq!(calls.take(), ["key a"]);
    // ↓ goes to the first item, the 보기 방식 menu, → opens it, ↓ goes to its first item, 큰
    // 아이콘, and Enter takes it.
    let record = calls.clone();
    window.on_command(move |name| record.note(format!("command {name}")));
    window.invoke_show_view_menu();
    press(&window, Key::DownArrow);
    press(&window, Key::RightArrow);
    press(&window, Key::DownArrow);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["command view-large-icons"]);
    // The sub-menu and the menu have closed: the panes take the keys again.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    chord(&window, &["b"]);
    assert_eq!(calls.take(), ["key b"]);
}

#[test]
fn the_failures_window_is_closed_and_scrolled_from_the_keys() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_dialog_cancelled(move || record.note("closed"));
    let lines: Vec<SharedString> = (1..=60)
        .map(|i| format!("항목 {i}.txt: 권한이 없습니다").into())
        .collect();
    let head: Vec<SharedString> = lines[..12].to_vec();
    window.set_message_view(MessageView {
        title: "처리하지 못한 항목".into(),
        lines: ModelRc::new(slint::VecModel::from(lines)),
        head: ModelRc::new(slint::VecModel::from(head)),
    });
    window.set_dialog(DialogKind::Message);
    // Where the second line is: the list makes only the lines it shows, and drops the first one
    // once it is mostly out of view.
    let line_y = || {
        labelled(&window, "항목 2.txt: 권한이 없습니다", AccessibleRole::Text)
            .first()
            .map(|line| line.absolute_position().y)
    };
    let top = line_y().expect("the second line shows");

    // ↓ and ↑ scroll the list by part of a line, Home back to its top; the window stays.
    press(&window, Key::DownArrow);
    assert_eq!(line_y(), Some(top - 24.0));
    press(&window, Key::UpArrow);
    assert_eq!(line_y(), Some(top));
    press(&window, Key::DownArrow);
    press(&window, Key::Home);
    assert_eq!(line_y(), Some(top));
    // PageDown scrolls by the list's height: the line goes out of view.
    press(&window, Key::PageDown);
    assert!(line_y().is_none_or(|y| y < top - 300.0), "{:?}", line_y());
    press(&window, Key::Home);
    assert_eq!(calls.take(), Vec::<String>::new());
    // Tab puts the keys on 확인, which Space presses; Enter and Esc close it too.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    press(&window, Key::Return);
    press(&window, Key::Escape);
    assert_eq!(calls.take(), ["closed", "closed", "closed"]);
}

#[test]
fn recovered_jobs_are_answered_from_the_keys_starting_on_close() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_recovery_chosen(move |i, again| record.note(format!("job {i} {again}")));
    let record = calls.clone();
    window.on_dialog_cancelled(move || record.note("closed"));
    let show = |rows: &[&str]| {
        let rows: Vec<SharedString> = rows.iter().map(|&row| row.into()).collect();
        window.set_recovery_view(RecoveryView {
            title: "끝나지 않은 작업".into(),
            note: "다시 실행하거나 버립니다".into(),
            rows: ModelRc::new(slint::VecModel::from(rows)),
        });
        window.set_dialog(DialogKind::Recovery);
        slint::platform::update_timers_and_animations();
    };
    show(&["복사 a → b", "이동 c → d"]);

    // The keys start on 닫기: Enter drops no job.
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["closed"]);
    // Tab goes round: the first job's 버리기, its 다시 실행, then the second job's.
    press(&window, Key::Tab);
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["job 0 true"]);
    // An answered job leaves the list, and the keys start again on 닫기: Shift+Tab goes back
    // to the last job's 다시 실행, and once more to its 버리기.
    show(&["이동 c → d"]);
    press_shifted(&window, Key::Tab);
    press_shifted(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["job 0 false"]);
    // Esc closes it from anywhere.
    press(&window, Key::Tab);
    press(&window, Key::Escape);
    assert_eq!(calls.take(), ["closed"]);
}

#[test]
fn the_name_window_tabs_between_its_field_and_its_buttons() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_name_edited(move |text| record.note(format!("edited {text}")));
    let record = calls.clone();
    window.on_name_accepted(move |text| record.note(format!("accepted {text}")));
    let record = calls.clone();
    window.on_dialog_cancelled(move || record.note("cancelled"));
    window.set_name_view(NameView {
        title: "새 폴더".into(),
        initial: "보고서".into(),
        select_end: 0,
        warning: "".into(),
        ok_enabled: true,
    });
    window.set_dialog(DialogKind::Name);
    slint::platform::update_timers_and_animations();

    // Tab: 취소, which Space presses.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["cancelled"]);
    // Once more: 확인, which Enter presses with the name; once more: the field, with the name
    // all selected, so that what is typed replaces it.
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["accepted 보고서"]);
    press(&window, Key::Tab);
    chord(&window, &["x"]);
    assert_eq!(calls.take(), ["edited x"]);
    // Shift+Tab from the field goes to 확인; letters there type nothing.
    press_shifted(&window, Key::Tab);
    chord(&window, &["y"]);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["accepted x"]);

    // While a syllable is composed in the field, Tab leaves it alone: the syllable is not in
    // the name yet.
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    compose(&window, "ㅅ");
    press(&window, Key::Tab);
    commit_composition(&window, "서");
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["edited x서", "accepted x서"]);
}

#[test]
fn the_notice_window_closes_from_the_keys_on_its_button() {
    i_slint_backend_testing::init_no_event_loop();
    let notice = NoticeWindow::new().expect("NoticeWindow builds");
    let closed = Rc::new(RefCell::new(0));
    let count = Rc::clone(&closed);
    notice.on_closed(move || *count.borrow_mut() += 1);
    notice.show().expect("the notice shows");
    let press = |key: &str| {
        notice
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        notice
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    };
    // Space, Enter and Esc press 확인; Tab keeps the keys on it.
    press(" ");
    press(&special(Key::Tab));
    press(&special(Key::Return));
    press(&special(Key::Escape));
    assert_eq!(*closed.borrow(), 3);
}

fn preflight_view(has_blocked: bool) -> PreflightView {
    let lines: Vec<SharedString> = vec!["a.txt: 이름이 너무 깁니다".into()];
    PreflightView {
        title: "복사하기 전에 확인할 것이 있습니다".into(),
        lines: ModelRc::new(slint::VecModel::from(lines.clone())),
        head: ModelRc::new(slint::VecModel::from(lines)),
        has_blocked,
    }
}

#[test]
fn the_preflight_window_starts_on_cancel() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_preflight_answered(move |choice| record.note(format!("answer {choice}")));
    let show = |has_blocked: bool| {
        window.set_dialog(DialogKind::None);
        window.set_preflight_view(preflight_view(has_blocked));
        window.set_dialog(DialogKind::Preflight);
        slint::platform::update_timers_and_animations();
    };

    // Enter on 취소 (2); Tab goes to 막힌 항목 건너뛰기 (1), then 계속 (0), then round to 취소.
    show(true);
    press(&window, Key::Return);
    press(&window, Key::Tab);
    press(&window, Key::Space);
    press(&window, Key::Tab);
    press(&window, Key::Return);
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(
        calls.take(),
        ["answer 2", "answer 1", "answer 0", "answer 2"]
    );
    // Without blocked items, Tab and Shift+Tab go between 취소 and 계속.
    show(false);
    press(&window, Key::Tab);
    press(&window, Key::Return);
    press_shifted(&window, Key::Tab);
    press(&window, Key::Return);
    press_shifted(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["answer 0", "answer 2", "answer 0"]);
}

#[test]
fn the_conflict_window_reaches_its_box_and_buttons_by_tab() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_conflict_answered(move |choice, all| record.note(format!("answer {choice} {all}")));
    let record = calls.clone();
    window.on_dialog_cancelled(move || record.note("cancelled"));
    window.set_conflict_view(conflict_view());
    window.set_dialog(DialogKind::Conflict);
    slint::platform::update_timers_and_animations();
    let apply_all = || {
        labelled(
            &window,
            "남은 충돌에도 같은 선택 적용",
            AccessibleRole::Checkbox,
        )[0]
        .accessible_checked()
    };

    // ↑↑ chooses overwrite, which Enter alone does not confirm.
    press(&window, Key::UpArrow);
    press(&window, Key::UpArrow);
    press(&window, Key::Return);
    assert_eq!(calls.take(), Vec::<String>::new());
    // Tab: the box, which Space flips; 작업 취소; the button, which confirms the overwrite.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(apply_all(), Some(true));
    press(&window, Key::Tab);
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["answer 0 true"]);
    // Shift+Tab back to 작업 취소, which Space presses.
    press_shifted(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["cancelled"]);

    // A new question starts again on the choices: Enter takes keeping both.
    window.set_conflict_view(ConflictView {
        serial: 2,
        ..conflict_view()
    });
    slint::platform::update_timers_and_animations();
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["answer 2 false"]);
}

#[test]
fn the_picker_tabs_round_its_parts() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_pick_filter_edited(move |text| record.note(format!("filter {text}")));
    let record = calls.clone();
    window.on_pick_path_edited(move |text| record.note(format!("path {text}")));
    let record = calls.clone();
    window.on_pick_new_folder(move || record.note("new folder"));
    let record = calls.clone();
    window.on_pick_mode(move |is_move| record.note(format!("move {is_move}")));
    let record = calls.clone();
    window.on_pick_policy(move |policy| record.note(format!("policy {policy}")));
    let record = calls.clone();
    window.on_pick_confirmed(move || record.note("confirmed"));
    let record = calls.clone();
    window.on_pick_enter(move || record.note("enter"));
    let record = calls.clone();
    window.on_dialog_cancelled(move || record.note("cancelled"));
    window.set_pick_view(pick_view());
    window.set_dialog(DialogKind::Pick);
    slint::platform::update_timers_and_animations();

    // The filter has the keys; Tab goes to the path field, whose text is all selected: what is
    // typed replaces it.
    chord(&window, &["백"]);
    press(&window, Key::Tab);
    chord(&window, &["/"]);
    assert_eq!(calls.take(), ["filter 백", "path /"]);
    // 새 폴더, which Enter presses.
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["new folder"]);
    // 이동/복사: → copies, ← moves.
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    press(&window, Key::LeftArrow);
    assert_eq!(calls.take(), ["move false", "move true"]);
    // 이름이 겹치면: → goes to 덮어쓰기; Enter there is the picker's Enter.
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["policy 1", "enter"]);
    // 취소, then the button: Space confirms (an overwrite too, since the button is pressed).
    press(&window, Key::Tab);
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["confirmed"]);
    // Round to the filter, whose text is all selected, and back with Shift+Tab to the button,
    // then 취소.
    press(&window, Key::Tab);
    chord(&window, &["업"]);
    assert_eq!(calls.take(), ["filter 업"]);
    press_shifted(&window, Key::Tab);
    press_shifted(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["cancelled"]);

    // A button that is off is passed over: without 새 폴더, Tab goes from the path to 이동/복사.
    window.set_pick_view(PickView {
        can_make_folder: false,
        ..pick_view()
    });
    // From 취소: the button, the filter, the path, 이동/복사.
    for _ in 0..4 {
        press(&window, Key::Tab);
    }
    press(&window, Key::RightArrow);
    assert_eq!(calls.take(), ["move false"]);
}

/// The place and the size of the pane called `label`; `None` while it does not show.
fn pane_box(window: &MainWindow, label: &str) -> Option<(LogicalPosition, LogicalSize)> {
    labelled(window, label, AccessibleRole::Groupbox)
        .first()
        .map(|pane| (pane.absolute_position(), pane.size()))
}

fn shown_pane(window: &MainWindow, label: &str) -> (LogicalPosition, LogicalSize) {
    pane_box(window, label).unwrap_or_else(|| panic!("the {label} shows"))
}

#[test]
fn the_divider_shares_the_room_and_a_double_click_evens_it() {
    let window = window();
    let shares = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&shares);
    window.on_split_changed(move |share| record.borrow_mut().push(share));
    let (left_at, left) = shown_pane(&window, "왼쪽 패널");
    let (right_at, right) = shown_pane(&window, "오른쪽 패널");
    assert!((left.width - right.width).abs() < 1.0, "even at the start");
    let gap = right_at.x - (left_at.x + left.width);
    let room = left.width + right.width;
    let middle_y = left_at.y + left.height / 2.0;
    let grip = LogicalPosition::new(left_at.x + left.width + gap / 2.0, middle_y);

    // Dragged to 30% of the room: the panes follow the pointer, and Rust is told at the
    // release.
    pointer(&window, "press", grip);
    let to = LogicalPosition::new(left_at.x + room * 0.3 + gap / 2.0, middle_y);
    for step in [1.0, 2.0, 3.0, 4.0] {
        let x = grip.x + (to.x - grip.x) * step / 4.0;
        pointer(&window, "move", LogicalPosition::new(x, middle_y));
    }
    let (_, dragged) = shown_pane(&window, "왼쪽 패널");
    assert!(
        (dragged.width - room * 0.3).abs() < 2.0,
        "{}",
        dragged.width
    );
    assert!(
        shares.borrow().is_empty(),
        "nothing is told before the release"
    );
    pointer(&window, "release", to);
    let told = shares.borrow().clone();
    assert_eq!(told.len(), 1);
    assert!((told[0] - 0.3).abs() < 0.01, "{told:?}");
    // Rust's share is the one shown after the release.
    window.set_split_share(told[0]);
    let (_, left) = shown_pane(&window, "왼쪽 패널");
    let (_, right) = shown_pane(&window, "오른쪽 패널");
    assert!((left.width / (left.width + right.width) - 0.3).abs() < 0.01);

    // Past the edge, the first pane keeps 200px (more than a tenth of this room).
    let grip = LogicalPosition::new(left_at.x + left.width + gap / 2.0, middle_y);
    pointer(&window, "press", grip);
    pointer(
        &window,
        "move",
        LogicalPosition::new(grip.x - 5.0, middle_y),
    );
    pointer(&window, "move", LogicalPosition::new(0.0, middle_y));
    pointer(&window, "release", LogicalPosition::new(0.0, middle_y));
    let last = *shares.borrow().last().expect("told");
    assert!((last * room - 200.0).abs() < 1.0, "{last} of {room}");

    // A double click asks for an even split.
    window.set_split_share(0.3);
    shares.borrow_mut().clear();
    pointer(&window, "move", grip);
    for _ in 0..2 {
        pointer(&window, "press", grip);
        pointer(&window, "release", grip);
    }
    assert_eq!(shares.borrow().as_slice(), [0.5]);
}

#[test]
fn panes_one_above_the_other_fit_the_smallest_window() {
    let window = window();
    window.window().set_size(LogicalSize::new(900.0, 560.0));
    window.set_stacked(true);
    let (top_at, top) = shown_pane(&window, "왼쪽 패널");
    let (bottom_at, bottom) = shown_pane(&window, "오른쪽 패널");
    assert!(
        bottom_at.y > top_at.y + top.height,
        "the right pane is below"
    );
    assert!((top_at.x - bottom_at.x).abs() < 1.0);
    assert!((top.width - bottom.width).abs() < 1.0);
    assert!((top.height - bottom.height).abs() < 1.0, "split evenly");
    // Each keeps room for its rows below its tab strip, path row and status bar (110px).
    assert!(top.height > 200.0, "{}", top.height);
    assert!(top.width > 500.0, "{}", top.width);
    assert!(bottom_at.y + bottom.height <= 560.0);
    // The divider lies between them, and a drag moves it up and down.
    let shares = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&shares);
    window.on_split_changed(move |share| record.borrow_mut().push(share));
    let gap = bottom_at.y - (top_at.y + top.height);
    let grip = LogicalPosition::new(
        top_at.x + top.width / 2.0,
        top_at.y + top.height + gap / 2.0,
    );
    drag_to(&window, grip, LogicalPosition::new(grip.x, grip.y + 40.0));
    pointer(
        &window,
        "release",
        LogicalPosition::new(grip.x, grip.y + 40.0),
    );
    let told = shares.borrow().clone();
    assert_eq!(told.len(), 1);
    assert!(told[0] > 0.5, "{told:?}");
}

#[test]
fn each_pane_keeps_200px_however_far_the_divider_goes() {
    let window = window();
    window.window().set_size(LogicalSize::new(900.0, 560.0));
    window.set_show_preview(true);
    let shares = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&shares);
    window.on_split_changed(move |share| record.borrow_mut().push(share));
    let (left_at, left) = shown_pane(&window, "왼쪽 패널");
    let (right_at, right) = shown_pane(&window, "오른쪽 패널");
    let room = left.width + right.width;
    let least = (room * 0.1).max(200.0_f32.min(room / 2.0));
    let middle_y = left_at.y + left.height / 2.0;
    let grip = LogicalPosition::new((left_at.x + left.width + right_at.x) / 2.0, middle_y);

    // Dragged to the window's left edge: the left pane keeps its least room.
    drag_to(&window, grip, LogicalPosition::new(0.0, middle_y));
    let (_, dragged) = shown_pane(&window, "왼쪽 패널");
    assert!(dragged.width >= least - 1.0, "{} of {room}", dragged.width);
    pointer(&window, "release", LogicalPosition::new(0.0, middle_y));
    // A share Rust keeps (from 10%) shows no narrower either, on both sides.
    for share in [0.1, 0.9] {
        window.set_split_share(share);
        let (_, left) = shown_pane(&window, "왼쪽 패널");
        let (_, right) = shown_pane(&window, "오른쪽 패널");
        assert!(
            left.width >= least - 1.0 && right.width >= least - 1.0,
            "{share}"
        );
    }
    assert_eq!(shares.borrow().len(), 1);
}

#[test]
fn one_pane_shows_only_the_active_one_with_the_whole_room() {
    let window = window();
    let (left_at, _) = shown_pane(&window, "왼쪽 패널");
    let (right_at, right) = shown_pane(&window, "오른쪽 패널");
    let whole = right_at.x + right.width - left_at.x;

    window.set_dual_pane(false);
    let (_, alone) = shown_pane(&window, "왼쪽 패널");
    assert!(pane_box(&window, "오른쪽 패널").is_none());
    assert!(
        (alone.width - whole).abs() < 1.0,
        "{} of {whole}",
        alone.width
    );

    // The right pane has the keyboard: it is the one that shows.
    window.set_right_pane(PaneView {
        active: true,
        ..PaneView::default()
    });
    assert!(pane_box(&window, "왼쪽 패널").is_none());
    let (at, alone) = shown_pane(&window, "오른쪽 패널");
    assert!((at.x - left_at.x).abs() < 1.0);
    assert!((alone.width - whole).abs() < 1.0);
}

#[test]
fn the_tree_and_the_preview_panel_come_and_go() {
    let window = window();
    let (wide_at, _) = shown_pane(&window, "왼쪽 패널");
    assert!(!labelled(&window, "폴더 트리", AccessibleRole::List).is_empty());
    assert!(pane_box(&window, "미리 보기").is_none());

    // Without the tree, the panes start where it was.
    window.set_show_tree(false);
    assert!(labelled(&window, "폴더 트리", AccessibleRole::List).is_empty());
    let (at, _) = shown_pane(&window, "왼쪽 패널");
    assert!(at.x < wide_at.x - 200.0, "{} and {}", at.x, wide_at.x);

    // The preview panel's place takes the right end.
    window.set_show_preview(true);
    let (preview_at, preview) = shown_pane(&window, "미리 보기");
    let (right_at, right) = shown_pane(&window, "오른쪽 패널");
    assert!(preview_at.x > right_at.x + right.width);
    assert!(preview.width >= 200.0);
    assert!(preview_at.x + preview.width <= 1280.0);
}

/// The default layout, as Rust sets it back after a checkable item of the 보기 menu flipped
/// its own mark.
fn default_layout(window: &MainWindow) {
    window.set_show_tree(true);
    window.set_show_preview(false);
    window.set_dual_pane(true);
    window.set_stacked(false);
    window.set_sync_navigation(false);
}

#[test]
fn the_view_menu_switches_the_layout() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_command(move |name| record.note(name.to_string()));
    let record = calls.clone();
    window.on_split_changed(move |share| record.note(format!("split {share:.2}")));
    let button = labelled(&window, "보기", AccessibleRole::Button).remove(0);
    // The menu opens, and the sub-menu `sub` of it ("" for none).
    let open = |sub: &str| {
        button.invoke_accessible_default_action();
        if !sub.is_empty() {
            choose_item(&window, sub);
        }
    };
    let choose = |sub: &str, item: &str| {
        open(sub);
        choose_item(&window, item);
        default_layout(&window);
    };
    for (sub, item, call) in [
        ("", "폴더 트리", "toggle-tree"),
        ("", "폴더 트리로 가기", "focus-tree"),
        ("", "미리 보기 패널", "toggle-preview"),
        ("배치", "듀얼 패널", "toggle-dual"),
        ("배치", "위아래로 나누기", "toggle-split"),
        ("배치", "두 패널을 같은 크기로", "split 0.50"),
        ("배치", "분할선을 왼쪽으로", "split 0.40"),
        ("배치", "분할선을 오른쪽으로", "split 0.60"),
        ("", "패널 바꾸기", "swap-panes"),
        ("", "같은 폴더를 반대편에", "same-folder"),
        ("", "탐색 동기화", "toggle-sync"),
    ] {
        choose(sub, item);
        assert_eq!(calls.take(), [call], "{item}");
    }
    // One above the other, the divider goes up and down.
    window.set_stacked(true);
    open("배치");
    for item in ["분할선을 위로", "분할선을 아래로"] {
        assert!(
            ElementHandle::find_by_accessible_label(&window, item)
                .next()
                .is_some(),
            "{item}"
        );
    }
    choose("테마", "밝게");
    calls.take();
    // With one pane, the items for the divider and the other pane do nothing.
    for (sub, item) in [
        ("배치", "두 패널을 같은 크기로"),
        ("배치", "분할선을 왼쪽으로"),
        ("", "같은 폴더를 반대편에"),
        ("", "탐색 동기화"),
    ] {
        window.set_dual_pane(false);
        open(sub);
        choose_item(&window, item);
        assert_eq!(calls.take(), Vec::<String>::new(), "{item}");
        // A disabled item leaves the menu open: Esc closes it (and the sub-menu first).
        press(&window, Key::Escape);
        if !sub.is_empty() {
            press(&window, Key::Escape);
        }
    }
    default_layout(&window);
    // The marks show the layout as Rust sets it back after each toggle: 폴더 트리, 미리 보기
    // 패널 and 탐색 동기화 in the menu, 듀얼 패널 and 위아래로 나누기 in its 배치 menu (and no
    // other: the extensions are not shown).
    window.set_show_extensions(false);
    let marks = || {
        open("배치");
        ElementHandle::find_by_accessible_label(&window, "✓").count()
    };
    for (tree, preview, dual, stacked, sync, count) in [
        (true, false, true, false, false, 2),
        (false, false, false, false, false, 0),
        (true, true, true, true, true, 5),
    ] {
        window.set_show_tree(tree);
        window.set_show_preview(preview);
        window.set_dual_pane(dual);
        window.set_stacked(stacked);
        window.set_sync_navigation(sync);
        assert_eq!(marks(), count, "{count} of the layout");
        press(&window, Key::Escape);
        press(&window, Key::Escape);
    }
}

#[test]
fn the_tree_takes_the_keys_until_it_gives_them_back() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    // The tree uses ↓ (as its arrows and Esc); F12 is the window's.
    let down = special(Key::DownArrow);
    window.on_tree_key(move |text, _, _, _, _| {
        let used = text == down.as_str();
        record.note(if used { "tree ↓" } else { "tree other" });
        used
    });
    let record = calls.clone();
    window.on_key(move |_, _, _, _, _| {
        record.note("key");
        true
    });
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["key"]);

    window.invoke_focus_tree();
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["tree ↓"]);
    // A key the tree does not use goes on to the window's keys.
    press(&window, Key::F12);
    assert_eq!(calls.take(), ["tree other", "key"]);

    // Back with the panes, the keys are theirs.
    window.invoke_focus_panes();
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["key"]);
}

/// One running job's row of the job panel.
fn job_row(title: &str) -> JobRowView {
    JobRowView {
        title: title.into(),
        detail: "assets → 백업".into(),
        progress: 0.25,
        progress_text: "3 / 12 항목".into(),
        paused: false,
        waiting: false,
    }
}

#[test]
fn the_job_panel_takes_the_keys_while_it_shows() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    // The panel uses ↓ (as its arrows, Space, Enter and Esc); F12 is the window's.
    let down = special(Key::DownArrow);
    window.on_panel_key(move |text, _, _, _, _| {
        let used = text == down.as_str();
        record.note(if used { "panel ↓" } else { "panel other" });
        used
    });
    let record = calls.clone();
    window.on_key(move |_, _, _, _, _| {
        record.note("key");
        true
    });
    // Without a job the panel does not show, and does not take the keys.
    window.invoke_focus_panel();
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["key"]);

    let rows = Rc::new(slint::VecModel::from(vec![job_row("복사 중")]));
    window.set_job_rows(ModelRc::from(Rc::clone(&rows)));
    window.set_panel_line(0);
    window.set_panel_button(1);
    window.invoke_focus_panel();
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["panel ↓"]);
    // A key the panel does not use goes on to the window's keys.
    press(&window, Key::F12);
    assert_eq!(calls.take(), ["panel other", "key"]);

    // The panel goes with its last job: the keys go back to the panes.
    rows.remove(0);
    // The window takes the keys from the panel as it goes; the panes take them a moment later.
    slint::platform::update_timers_and_animations();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["key"]);
}

#[test]
fn the_view_menu_gives_the_tree_the_keys() {
    let window = window();
    let calls = Calls::default();
    // As the controller does: the command gives the tree the keys.
    let weak = window.as_weak();
    window.on_command(move |name| {
        if name == "focus-tree"
            && let Some(window) = weak.upgrade()
        {
            window.invoke_focus_tree();
        }
    });
    let record = calls.clone();
    window.on_tree_key(move |_, _, _, _, _| {
        record.note("tree");
        true
    });
    let record = calls.clone();
    window.on_key(move |_, _, _, _, _| {
        record.note("key");
        true
    });

    let button = labelled(&window, "보기", AccessibleRole::Button).remove(0);
    button.invoke_accessible_default_action();
    ElementHandle::find_by_accessible_label(&window, "폴더 트리로 가기")
        .next()
        .expect("the menu shows it")
        .mock_single_click(slint::platform::PointerEventButton::Left);
    // The menu has closed, giving the keys back to what had them; the tree takes them then.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    slint::platform::update_timers_and_animations();

    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["tree"]);

    // A choice in a sub-menu, opened from the tree, gives the keys back to the tree.
    window.invoke_show_view_menu();
    for _ in 0..4 {
        press(&window, Key::DownArrow);
    }
    press(&window, Key::RightArrow);
    press(&window, Key::DownArrow);
    press(&window, Key::Return);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["tree"]);
    // From the panes, back to the panes.
    window.invoke_focus_panes();
    window.invoke_show_view_menu();
    for _ in 0..4 {
        press(&window, Key::DownArrow);
    }
    press(&window, Key::RightArrow);
    press(&window, Key::DownArrow);
    press(&window, Key::Return);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    press(&window, Key::DownArrow);
    assert_eq!(calls.take(), ["key"]);
}

#[test]
fn the_view_menu_s_order_items_show_the_key_and_the_direction() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_sort(move |side, column, then| record.note(format!("sort {side} {column} {then}")));
    let record = calls.clone();
    window.on_sort_then(move |side, column| record.note(format!("then {side} {column}")));
    // The left pane by size, descending, then by name, ascending.
    let mut pane = listed_pane(vec![entry("a.txt", EntryKind::File, 10)]);
    let mut view = FolderView::default();
    view.sort_by(rf_vfs::SortKey::Size, false);
    view.sort_by(rf_vfs::SortKey::Size, false);
    view.set_then(Some(rf_vfs::SortKey::Name));
    pane.active_tab_mut().set_view(view);
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));
    let button = labelled(&window, "보기", AccessibleRole::Button).remove(0);
    let open = |path: &[&str]| {
        button.invoke_accessible_default_action();
        for item in path {
            choose_item(&window, item);
        }
    };
    let marked = |title: &str| marked(&window, title);
    open(&["정렬"]);
    assert!(marked("크기"));
    assert!(marked("내림차순"));
    assert!(!marked("오름차순"));
    assert!(marked("폴더 먼저"));
    // The direction chosen: the key's head clicked once more; the one it has already, nothing.
    choose_item(&window, "오름차순");
    assert_eq!(calls.take(), ["sort 0 1 false"]);
    open(&["정렬"]);
    choose_item(&window, "내림차순");
    assert_eq!(calls.take(), Vec::<String>::new());
    open(&["정렬", "2차 정렬"]);
    assert!(marked("이름"));
    assert!(!marked("없음"));
    // The second key's direction: its head clicked with Shift (the sub-menu's item, the last
    // of the two that show).
    ElementHandle::find_by_accessible_label(&window, "내림차순")
        .last()
        .expect("the sub-menu shows it")
        .mock_single_click(slint::platform::PointerEventButton::Left);
    assert_eq!(calls.take(), ["sort 0 0 true"]);
}

/// Whether the item `title` of a menu that shows has its check mark: a menu item's `✓` comes
/// just before its title among the texts.
fn marked(window: &MainWindow, title: &str) -> bool {
    let texts: Vec<SharedString> = i_slint_backend_testing::ElementQuery::from_root(window)
        .match_accessible_role(AccessibleRole::Text)
        .find_all()
        .iter()
        .map(|text| text.accessible_label().unwrap_or_default())
        .collect();
    texts
        .windows(2)
        .any(|pair| pair[0] == "✓" && pair[1] == title)
}

/// The pattern window as Rust shows it: the last pattern, `*.jpg`, as wildcards ignoring case,
/// in place of the selection.
fn pattern_view(count: &str, problem: &str, ok_enabled: bool) -> PatternView {
    PatternView {
        initial: "*.jpg".into(),
        regex: false,
        ignore_case: true,
        add: false,
        count: count.into(),
        problem: problem.into(),
        ok_enabled,
    }
}

/// A window that shows `view` in the pattern window and notes each of its calls.
fn pattern_window(view: PatternView) -> (MainWindow, Calls) {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_pattern_edited(move |text| record.note(format!("edited {text}")));
    let record = calls.clone();
    window.on_pattern_accepted(move |text| record.note(format!("accepted {text}")));
    let record = calls.clone();
    window.on_pattern_syntax(move |regex| record.note(format!("regex {regex}")));
    let record = calls.clone();
    window.on_pattern_case(move |ignore| record.note(format!("ignore case {ignore}")));
    let record = calls.clone();
    window.on_pattern_mode(move |add| record.note(format!("add {add}")));
    let record = calls.clone();
    window.on_dialog_cancelled(move || record.note("cancelled"));
    let record = calls.clone();
    window.on_key(move |text, _, _, _, _| {
        record.note(format!("key {text}"));
        true
    });
    window.set_pattern_view(view);
    window.set_dialog(DialogKind::Pattern);
    slint::platform::update_timers_and_animations();
    (window, calls)
}

/// The pattern window's 선택: after the toolbar's 선택 under the scrim.
fn select_button(window: &MainWindow) -> ElementHandle {
    labelled(window, "선택", AccessibleRole::Button)
        .pop()
        .expect("the window's 선택")
}

#[test]
fn the_pattern_window_shows_its_count_and_hands_the_pattern_over() {
    let (window, calls) = pattern_window(pattern_view("맞는 항목 3개", "", true));
    assert_eq!(
        labelled(&window, "맞는 항목 3개", AccessibleRole::Text).len(),
        1
    );
    let field = || labelled(&window, "패턴", AccessibleRole::TextInput).remove(0);
    // The last pattern, all selected: what is typed replaces it.
    assert_eq!(field().accessible_value().as_deref(), Some("*.jpg"));
    chord(&window, &["*"]);
    chord(&window, &["."]);
    chord(&window, &["p"]);
    assert_eq!(calls.take(), ["edited *", "edited *.", "edited *.p"]);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["accepted *.p"]);
    select_button(&window).invoke_accessible_default_action();
    assert_eq!(calls.take(), ["accepted *.p"]);
    // The choices are radio buttons and a box, each named.
    labelled(&window, "정규식", AccessibleRole::RadioButton)[0].invoke_accessible_default_action();
    labelled(&window, "대소문자 무시", AccessibleRole::Checkbox)[0]
        .invoke_accessible_default_action();
    labelled(&window, "선택에 더하기", AccessibleRole::RadioButton)[0]
        .invoke_accessible_default_action();
    assert_eq!(
        calls.take(),
        ["regex true", "ignore case false", "add true"]
    );
    // Esc closes it; no key reached the panes meanwhile.
    press(&window, Key::Escape);
    assert_eq!(calls.take(), ["cancelled"]);
}

#[test]
fn a_pattern_that_is_no_pattern_says_why_and_holds_the_button() {
    let (window, calls) = pattern_window(pattern_view("", "올바른 정규식이 아닙니다", false));
    assert_eq!(
        labelled(&window, "올바른 정규식이 아닙니다", AccessibleRole::Text).len(),
        1
    );
    assert_eq!(select_button(&window).accessible_enabled(), Some(false));
    select_button(&window).invoke_accessible_default_action();
    press(&window, Key::Return);
    // Tab passes the button over: from 취소 back to the field.
    for _ in 0..5 {
        press(&window, Key::Tab);
    }
    chord(&window, &["x"]);
    assert_eq!(calls.take(), ["edited x"]);
}

#[test]
fn a_pattern_being_composed_holds_the_pattern_window() {
    let (window, calls) = pattern_window(pattern_view("맞는 항목 3개", "", true));
    let field = || labelled(&window, "패턴", AccessibleRole::TextInput).remove(0);
    // The cursor after the text, as after a click there.
    field().mock_single_click(slint::platform::PointerEventButton::Left);
    press(&window, Key::RightArrow);
    // While a syllable is composed, Rust has the pattern before it: neither the button, nor
    // Enter, nor Tab takes it.
    compose(&window, "ㅅ");
    slint::platform::update_timers_and_animations();
    assert_eq!(field().accessible_value().as_deref(), Some("*.jpg"));
    assert_eq!(select_button(&window).accessible_enabled(), Some(false));
    select_button(&window).invoke_accessible_default_action();
    press(&window, Key::Return);
    press(&window, Key::Tab);
    assert_eq!(calls.take(), Vec::<String>::new());
    // Committed, the syllable is part of the pattern, which Enter hands over.
    commit_composition(&window, "사");
    slint::platform::update_timers_and_animations();
    assert_eq!(select_button(&window).accessible_enabled(), Some(true));
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["edited *.jpg사", "accepted *.jpg사"]);
}

#[test]
fn the_pattern_window_s_choices_take_the_keys_in_turn() {
    let (window, calls) = pattern_window(pattern_view("맞는 항목 3개", "", true));
    // Tab: 와일드카드/정규식, where ← → choose.
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    press(&window, Key::LeftArrow);
    // Tab: 대소문자 무시, which Space turns off.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    // Tab: 선택 바꾸기/선택에 더하기.
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    assert_eq!(
        calls.take(),
        ["regex true", "regex false", "ignore case false", "add true"]
    );
    // Tab: 취소, then 선택, which Space presses with the field's text.
    press(&window, Key::Tab);
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["accepted *.jpg"]);
    // Shift+Tab back to 취소, which Enter presses.
    press_shifted(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["cancelled"]);
}

/// The properties window of one file, `a.txt`, its count of a folder under way when
/// `counting`, and its SHA-256 made when `algorithm` is 2.
fn properties_view(counting: bool, algorithm: i32) -> PropertiesView {
    let facts = [
        ("종류", "텍스트 문서", ""),
        ("위치", "/…/사진/2026", "/home/me/아주 긴 폴더/사진/2026"),
        ("크기", "3 바이트", ""),
        ("속성", "읽기 전용", ""),
    ]
    .map(|(label, value, full)| FactRow {
        label: label.into(),
        value: value.into(),
        full: full.into(),
    });
    let hash = HashRowView {
        name: "a.txt".into(),
        value: if algorithm == 2 {
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()
        } else {
            SharedString::new()
        },
        progress: -1.0,
        done: algorithm == 2,
        copied: false,
        matched: false,
    };
    PropertiesView {
        title: "a.txt".into(),
        icon: "document".into(),
        folder: false,
        facts: ModelRc::new(slint::VecModel::from(facts.to_vec())),
        counting,
        count_note: if counting {
            "세는 중…".into()
        } else {
            SharedString::new()
        },
        hashable: true,
        hashing: false,
        algorithm,
        hash_note: SharedString::new(),
        several: false,
        // OS 속성 창 열기 has a test of its own.
        system: false,
        hashes: ModelRc::new(slint::VecModel::from(vec![hash])),
        verdict: SharedString::new(),
        matched: false,
        mismatched: false,
    }
}

/// A window that shows `view` in the properties window and notes each of its calls.
fn properties_window(view: PropertiesView) -> (MainWindow, Calls) {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_properties_algorithm(move |number| record.note(format!("algorithm {number}")));
    let record = calls.clone();
    window.on_properties_stop(move || record.note("stop"));
    let record = calls.clone();
    window.on_properties_stop_hashes(move || record.note("stop hashes"));
    let record = calls.clone();
    window.on_properties_copy(move |row| record.note(format!("copy {row}")));
    let record = calls.clone();
    window.on_properties_compare(move |text| record.note(format!("compare {text}")));
    let record = calls.clone();
    window.on_properties_system(move || record.note("system"));
    let record = calls.clone();
    window.on_dialog_cancelled(move || record.note("cancelled"));
    let record = calls.clone();
    window.on_key(move |text, _, _, _, _| {
        record.note(format!("key {text}"));
        true
    });
    window.set_properties_view(view);
    window.set_dialog(DialogKind::Properties);
    slint::platform::update_timers_and_animations();
    (window, calls)
}

#[test]
fn the_properties_window_shows_its_facts_and_hands_the_choices_over() {
    let (window, calls) = properties_window(properties_view(false, -1));
    for text in ["a.txt", "텍스트 문서", "3 바이트", "읽기 전용"] {
        assert_eq!(
            labelled(&window, text, AccessibleRole::Text).len(),
            1,
            "{text}"
        );
    }
    // A shortened path gives screen readers the whole of it (and the pointer a tooltip).
    let place = labelled(&window, "/…/사진/2026", AccessibleRole::Text).remove(0);
    assert_eq!(
        place.accessible_description().as_deref(),
        Some("/home/me/아주 긴 폴더/사진/2026")
    );
    // No checksum before an algorithm is chosen, and nothing to copy.
    assert!(labelled(&window, "복사", AccessibleRole::Button).is_empty());
    labelled(&window, "SHA-256", AccessibleRole::RadioButton)[0].invoke_accessible_default_action();
    assert_eq!(calls.take(), ["algorithm 2"]);
    // The checksum came: 복사 copies it.
    window.set_properties_view(properties_view(false, 2));
    slint::platform::update_timers_and_animations();
    labelled(&window, "복사", AccessibleRole::Button)
        .pop()
        .expect("the row's 복사")
        .invoke_accessible_default_action();
    assert_eq!(calls.take(), ["copy 0"]);
    // Enter, with no button chosen, closes it; no key reached the panes.
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["cancelled"]);
    press(&window, Key::Escape);
    assert_eq!(calls.take(), ["cancelled"]);
}

#[test]
fn the_properties_window_s_stops_take_the_keys_in_turn() {
    let (window, calls) = properties_window(properties_view(true, 2));
    // Tab: 중지, which Space presses.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["stop"]);
    // Tab: the algorithms, where ← → choose.
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    press(&window, Key::LeftArrow);
    assert_eq!(calls.take(), ["algorithm 3", "algorithm 1"]);
    // Tab: the checksum, which Enter copies.
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["copy 0"]);
    // Tab: 비교, which takes what is typed.
    press(&window, Key::Tab);
    chord(&window, &["F"]);
    assert_eq!(calls.take(), ["compare F"]);
    // Tab: 닫기, which Space presses; Shift+Tab goes back to 비교.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["cancelled"]);
    press_shifted(&window, Key::Tab);
    chord(&window, &["0"]);
    assert_eq!(calls.take(), ["compare 0"]);
    // The count ended: Tab from 닫기 passes 중지 over, to the algorithms.
    window.set_properties_view(properties_view(false, 2));
    press(&window, Key::Tab);
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    assert_eq!(calls.take(), ["algorithm 3"]);
}

#[test]
fn checksums_under_way_can_be_stopped_and_made_again_from_the_keys() {
    let mut view = properties_view(false, 2);
    view.hashing = true;
    let (window, calls) = properties_window(view);
    // Tab: the algorithms, where Space chooses the one chosen again.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["algorithm 2"]);
    // Tab: the checksum, which Enter copies.
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["copy 0"]);
    // Tab: 계산 중지, which Space presses; a click does the same.
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["stop hashes"]);
    labelled(&window, "계산 중지", AccessibleRole::Button)[0].invoke_accessible_default_action();
    assert_eq!(calls.take(), ["stop hashes"]);
    // Still on 계산 중지, the checksums end: it is gone, the keys are nowhere, and Tab goes
    // from the first stop that shows.
    window.set_properties_view(properties_view(false, 2));
    assert!(labelled(&window, "계산 중지", AccessibleRole::Button).is_empty());
    press(&window, Key::Tab);
    press(&window, Key::RightArrow);
    assert_eq!(calls.take(), ["algorithm 3"]);
}

#[test]
fn a_hash_being_composed_holds_the_compare_field() {
    let (window, calls) = properties_window(properties_view(false, 2));
    let field = || labelled(&window, "비교", AccessibleRole::TextInput).remove(0);
    field().mock_single_click(slint::platform::PointerEventButton::Left);
    chord(&window, &["a"]);
    assert_eq!(calls.take(), ["compare a"]);
    // While a syllable is composed, Rust compares the text before it, and neither Tab nor
    // Enter goes anywhere with the syllable left behind.
    compose(&window, "ㅂ");
    slint::platform::update_timers_and_animations();
    assert_eq!(field().accessible_value().as_deref(), Some("a"));
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), Vec::<String>::new());
    commit_composition(&window, "비");
    slint::platform::update_timers_and_animations();
    assert_eq!(calls.take(), ["compare a비"]);
    // Enter in the field closes nothing: the verdict shows as it is typed.
    press(&window, Key::Return);
    assert_eq!(calls.take(), Vec::<String>::new());
    // Committed, Tab leaves it for 닫기.
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["cancelled"]);
}

#[test]
fn the_menus_end_with_the_properties() {
    use slint::platform::PointerEventButton;
    let (window, _calls, _models, _tree) = drag_window();
    let commands = Calls::default();
    let record = commands.clone();
    window.on_command(move |name| record.note(name.to_string()));
    for (os, item) in [
        (Os::Windows, "속성  (Alt+Enter)"),
        (Os::MacOs, "속성  (⌘I)"),
    ] {
        ui_bridge::show_shortcuts(&window, os);
        labelled(&window, "x.txt", AccessibleRole::ListItem)
            .remove(0)
            .mock_single_click(PointerEventButton::Right);
        choose_item(&window, item);
        assert_eq!(commands.take(), ["properties"], "{os:?}");
        // The part without rows has the folder's own, without a key.
        labelled(&window, "항목 목록", AccessibleRole::List)
            .remove(0)
            .mock_single_click(PointerEventButton::Right);
        choose_item(&window, "속성");
        assert_eq!(commands.take(), ["folder-properties"], "{os:?}");
    }
}

#[test]
fn the_properties_window_of_one_item_opens_the_system_s_own() {
    let mut view = properties_view(false, 2);
    view.system = true;
    let (window, calls) = properties_window(view);
    labelled(&window, "OS 속성 창 열기", AccessibleRole::Button)
        .remove(0)
        .invoke_accessible_default_action();
    assert_eq!(calls.take(), ["system"]);
    // From the keys: after 비교, before 닫기.
    let field = || labelled(&window, "비교", AccessibleRole::TextInput).remove(0);
    field().mock_single_click(slint::platform::PointerEventButton::Left);
    press(&window, Key::Tab);
    press(&window, Key::Return);
    assert_eq!(calls.take(), ["system"]);
    press(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["cancelled"]);
    press_shifted(&window, Key::Tab);
    press(&window, Key::Space);
    assert_eq!(calls.take(), ["system"]);
    // A window of several items has none.
    let mut several = properties_view(false, 2);
    several.system = false;
    window.set_properties_view(several);
    slint::platform::update_timers_and_animations();
    assert!(labelled(&window, "OS 속성 창 열기", AccessibleRole::Button).is_empty());
}

/// What the item menu offers for one file, one program, one folder, and several items.
const ONE_FILE: MenuFacts = MenuFacts {
    one_file: true,
    runnable: false,
    one_folder: false,
    one_item: true,
};
const ONE_PROGRAM: MenuFacts = MenuFacts {
    runnable: true,
    ..ONE_FILE
};
const ONE_FOLDER: MenuFacts = MenuFacts {
    one_file: false,
    runnable: false,
    one_folder: true,
    one_item: true,
};
const SEVERAL: MenuFacts = MenuFacts {
    one_file: false,
    runnable: false,
    one_folder: false,
    one_item: false,
};

/// Sets what the item menu offers, as `AppState::item_menu` says it.
fn offer(window: &MainWindow, facts: MenuFacts) {
    ui_bridge::show_item_menu(window, facts);
}

#[test]
fn the_item_menu_offers_the_system_s_ways_to_open_an_item() {
    use slint::platform::PointerEventButton;
    let (window, _calls, _models, _tree) = drag_window();
    let commands = Calls::default();
    let record = commands.clone();
    window.on_command(move |name| record.note(name.to_string()));
    let shows = |title: &str| {
        ElementHandle::find_by_accessible_label(&window, title)
            .next()
            .is_some()
    };
    let row_menu = |name: &str| {
        labelled(&window, name, AccessibleRole::ListItem)
            .remove(0)
            .mock_single_click(PointerEventButton::Right);
    };
    ui_bridge::show_shortcuts(&window, Os::Windows);

    // One program, on Windows.
    offer(&window, ONE_PROGRAM);
    for (item, command) in [
        ("관리자 권한으로 실행", "run-as-admin"),
        ("연결 프로그램으로 열기…", "open-with"),
        ("OS 속성 창 열기", "system-properties"),
    ] {
        row_menu("x.txt");
        assert!(!shows("터미널에서 열기  (Shift+F4)"), "not for a file");
        choose_item(&window, item);
        assert_eq!(commands.take(), [command], "{item}");
    }
    // One folder: a terminal there, and no app to open it with.
    offer(&window, ONE_FOLDER);
    row_menu("docs");
    assert!(!shows("연결 프로그램으로 열기…"));
    assert!(!shows("관리자 권한으로 실행"));
    choose_item(&window, "터미널에서 열기  (Shift+F4)");
    assert_eq!(commands.take(), ["open-terminal"]);
    // Several items: none of them.
    offer(&window, SEVERAL);
    row_menu("x.txt");
    for item in [
        "관리자 권한으로 실행",
        "연결 프로그램으로 열기…",
        "터미널에서 열기  (Shift+F4)",
        "OS 속성 창 열기",
    ] {
        assert!(!shows(item), "{item}");
    }
    choose_item(&window, "열기  (Enter)");
    assert_eq!(commands.take(), ["open"]);
    // macOS writes the key its way.
    ui_bridge::show_shortcuts(&window, Os::MacOs);
    offer(&window, ONE_FOLDER);
    row_menu("docs");
    choose_item(&window, "터미널에서 열기  (⇧F4)");
    assert_eq!(commands.take(), ["open-terminal"]);
    // The part without rows: a terminal in the pane's folder, without a key.
    labelled(&window, "항목 목록", AccessibleRole::List)
        .remove(0)
        .mock_single_click(PointerEventButton::Right);
    choose_item(&window, "터미널에서 열기");
    assert_eq!(commands.take(), ["folder-terminal"]);
}

#[test]
fn the_keys_open_the_item_menu_by_its_row_and_the_folder_s_menu() {
    let (window, _calls, models, _tree) = drag_window();
    let commands = Calls::default();
    let record = commands.clone();
    window.on_command(move |name| record.note(name.to_string()));
    ui_bridge::show_shortcuts(&window, Os::Windows);
    offer(&window, ONE_FILE);
    let at = || {
        let menu = window.global::<rf_ui::ItemMenu<'_>>();
        (menu.get_at_x(), menu.get_at_y())
    };
    for mode in [ViewMode::Details, ViewMode::MediumIcons, ViewMode::List] {
        left_in(&window, &models, mode);
        // Row 1 (x.txt), as Shift+F10 asks for it.
        window.invoke_show_item_menu(0, 1);
        choose_item(&window, "연결 프로그램으로 열기…");
        assert_eq!(commands.take(), ["open-with"], "{mode:?}");
        // It noted where it showed, inside the left pane, for the menu of apps.
        let (x, y) = at();
        let pane = labelled(&window, "왼쪽 패널", AccessibleRole::Groupbox);
        let (origin, size) = pane
            .first()
            .map(|pane| (pane.absolute_position(), pane.size()))
            .expect("the left pane");
        assert!(
            x > origin.x && x < origin.x + size.width && y > origin.y && y < origin.y + size.height,
            "{mode:?}: ({x}, {y}) outside the pane"
        );
        // Nothing chosen: the folder's menu.
        window.invoke_show_blank_menu(0);
        choose_item(&window, "새로 고침  (F5)");
        assert_eq!(commands.take(), ["refresh"], "{mode:?}");
    }
}

#[test]
fn the_menu_of_apps_opens_the_file_with_the_one_chosen() {
    let window = window();
    let chosen = Calls::default();
    let record = chosen.clone();
    window.on_open_with_chosen(move |index| record.note(format!("app {index}")));
    window.set_open_with_apps(ModelRc::new(slint::VecModel::from(vec![
        SharedString::from("미리보기 (기본)"),
        SharedString::from("TextEdit"),
    ])));
    window.invoke_show_open_with();
    choose_item(&window, "TextEdit");
    assert_eq!(chosen.take(), ["app 1"]);
    window.invoke_show_open_with();
    choose_item(&window, "미리보기 (기본)");
    assert_eq!(chosen.take(), ["app 0"]);
}

#[test]
fn the_select_menu_says_its_keys_and_runs_the_commands() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_command(move |name| record.note(name.to_string()));
    let button = labelled(&window, "선택", AccessibleRole::Button).remove(0);
    for (os, items) in [
        (
            Os::Windows,
            [
                "모두 선택  (Ctrl+A)",
                "선택 해제  (Esc)",
                "선택 반전  (Ctrl+Shift+I)",
                "패턴으로 선택…  (Ctrl+S)",
                "같은 확장자 선택  (Ctrl+Shift+S)",
            ],
        ),
        (
            Os::MacOs,
            [
                "모두 선택  (⌘A)",
                "선택 해제  (⎋)",
                "선택 반전  (⌥⇧⌘A)",
                "패턴으로 선택…  (⌘S)",
                "같은 확장자 선택  (⇧⌘S)",
            ],
        ),
    ] {
        ui_bridge::show_shortcuts(&window, os);
        for item in items {
            button.invoke_accessible_default_action();
            choose_item(&window, item);
        }
        assert_eq!(
            calls.take(),
            [
                "select-all",
                "clear-selection",
                "invert-selection",
                "select-pattern",
                "select-same-extension"
            ],
            "{os:?}"
        );
    }
}

#[test]
fn the_view_menu_shows_and_hides_the_extensions() {
    let window = window();
    let calls = Calls::default();
    let record = calls.clone();
    window.on_command(move |name| record.note(name.to_string()));
    let button = labelled(&window, "보기", AccessibleRole::Button).remove(0);
    button.invoke_accessible_default_action();
    // On by default, as the names show them.
    assert!(marked(&window, "파일 확장명 보기"));
    choose_item(&window, "파일 확장명 보기");
    assert_eq!(calls.take(), ["toggle-extensions"]);
    window.set_show_extensions(false);
    button.invoke_accessible_default_action();
    assert!(!marked(&window, "파일 확장명 보기"));
    assert_eq!(
        labelled(&window, "파일 확장명 보기", AccessibleRole::Text).len(),
        1
    );
    press(&window, Key::Escape);
}

#[test]
fn hidden_extensions_leave_the_names_before_them_in_every_view() {
    let window = window();
    let mut pane = listed_pane(vec![
        entry("photos.2026", EntryKind::Dir, 0),
        entry("앨범.2025", EntryKind::Symlink, 0),
        entry(".bashrc", EntryKind::File, 1),
        entry("설계안.pdf", EntryKind::File, 10),
    ]);
    let models = PaneModels::default();
    models.set_show_extensions(false);
    for mode in [
        ViewMode::Details,
        ViewMode::List,
        ViewMode::SmallIcons,
        ViewMode::MediumIcons,
        ViewMode::LargeIcons,
    ] {
        pane.active_tab_mut().set_view(FolderView {
            mode,
            ..FolderView::default()
        });
        window.set_left_pane(models.view(&pane, true));
        let shown = |name: &str| labelled(&window, name, AccessibleRole::ListItem).len();
        assert_eq!(shown("설계안"), 1, "{mode:?}");
        assert_eq!(shown("설계안.pdf"), 0, "{mode:?}");
        // A folder's name and a dot file's have no extension to leave out, and a link's shows
        // whole (it may lead to a folder).
        assert_eq!(shown("photos.2026"), 1, "{mode:?}");
        assert_eq!(shown("앨범.2025"), 1, "{mode:?}");
        assert_eq!(shown(".bashrc"), 1, "{mode:?}");
    }
    // Shown again: the whole names, at once.
    models.set_show_extensions(true);
    window.set_left_pane(models.view(&pane, true));
    assert_eq!(
        labelled(&window, "설계안.pdf", AccessibleRole::ListItem).len(),
        1
    );
}

#[test]
fn a_list_listed_again_stays_where_it_was_scrolled() {
    let window = window();
    let rows = |modified: Option<std::time::SystemTime>| -> Vec<Entry> {
        (0..3_000)
            .map(|i| {
                let mut row = entry(&format!("f{i:04}"), EntryKind::File, 1);
                row.modified = modified;
                row
            })
            .collect()
    };
    let mut pane = listed_pane(rows(None));
    let models = PaneModels::default();
    window.set_left_pane(models.view(&pane, true));
    window.invoke_scroll_to(0, 2_000);
    let place = |name: &str| {
        labelled(&window, name, AccessibleRole::ListItem)
            .first()
            .map(|row| row.absolute_position().y)
    };
    let before = place("f2000").expect("scrolled to it");
    assert_eq!(place("f0001"), None);
    // F5: the folder is listed again, and every row's date has changed (more rows than the list
    // is told one by one: it starts over its rows). It stays where it was.
    let request = pane.active_tab_mut().reload();
    let handled = pane.handle(LoadEvent::Listed {
        generation: request.generation,
        entries: rows(Some(
            std::time::UNIX_EPOCH + std::time::Duration::from_hours(1),
        )),
    });
    assert!(matches!(handled, rf_ui::pane::Handled::Applied { .. }));
    window.set_left_pane(models.view(&pane, true));
    assert_eq!(place("f2000"), Some(before));
    assert_eq!(place("f0001"), None);
}

#[test]
fn the_menu_of_a_list_s_empty_part_lists_the_folder_again_in_every_view() {
    let window = window();
    ui_bridge::show_shortcuts(&window, Os::Windows);
    let calls = Calls::default();
    let record = calls.clone();
    window.on_command(move |name| record.note(format!("command {name}")));
    let record = calls.clone();
    window.on_pane_activated(move |side| record.note(format!("pane {side}")));
    let mut pane = listed_pane(vec![entry("a.txt", EntryKind::File, 1)]);
    let models = PaneModels::default();
    for mode in [ViewMode::Details, ViewMode::MediumIcons, ViewMode::List] {
        pane.active_tab_mut().set_view(FolderView {
            mode,
            ..FolderView::default()
        });
        window.set_left_pane(models.view(&pane, true));
        // A right press well below the one item.
        let item = labelled(&window, "a.txt", AccessibleRole::ListItem).remove(0);
        let at = item.absolute_position();
        let position = LogicalPosition::new(at.x + 4.0, at.y + 300.0);
        let button = slint::platform::PointerEventButton::Right;
        window
            .window()
            .dispatch_event(WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(WindowEvent::PointerReleased { position, button });
        choose_item(&window, "새로 고침  (F5)");
        assert_eq!(calls.take(), ["pane 0", "command refresh"], "{mode:?}");
    }
}

/// A preview of `name` of `kind`, with `note`.
fn preview_of(kind: PreviewKind, name: &str, note: &str) -> PreviewView {
    PreviewView {
        kind,
        name: name.into(),
        icon: "file".into(),
        facts: "파일 · 3 KB · 2026-10-30 09:12".into(),
        note: note.into(),
        ..PreviewView::default()
    }
}

fn picture(width: u32, height: u32) -> slint::Image {
    slint::Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(
        width, height,
    ))
}

fn lines(texts: &[&str]) -> ModelRc<SharedString> {
    let lines: Vec<SharedString> = texts.iter().map(|text| (*text).into()).collect();
    ModelRc::new(slint::VecModel::from(lines))
}

#[test]
fn the_preview_panel_shows_an_image_a_text_and_why_it_shows_nothing() {
    let window = window();
    window.set_show_preview(true);
    let panel = labelled(&window, "미리 보기", AccessibleRole::Groupbox);
    assert_eq!(panel.len(), 1);
    let panel_width = panel[0].size().width;
    assert_eq!(
        labelled(&window, "미리 볼 항목이 없습니다", AccessibleRole::Text).len(),
        1
    );

    // An image as large as it is, when it fits.
    window.set_preview(PreviewView {
        image: picture(40, 20),
        ..preview_of(PreviewKind::Image, "사진.png", "40 × 20")
    });
    let image = labelled(&window, "사진.png", AccessibleRole::Image);
    assert_eq!(image.len(), 1);
    assert_eq!(image[0].size(), LogicalSize::new(40.0, 20.0));
    assert_eq!(labelled(&window, "40 × 20", AccessibleRole::Text).len(), 1);
    // A wide one fits the panel, its sides as they were.
    window.set_preview(PreviewView {
        image: picture(1600, 400),
        ..preview_of(PreviewKind::Image, "넓은.png", "1600 × 400")
    });
    let size = labelled(&window, "넓은.png", AccessibleRole::Image)[0].size();
    assert!(size.width < panel_width, "{size:?} in {panel_width}");
    assert!((size.width / size.height - 4.0).abs() < 0.05, "{size:?}");

    // A text, its lines and its encoding.
    window.set_preview(PreviewView {
        lines: lines(&["첫 줄입니다", "둘째 줄"]),
        ..preview_of(PreviewKind::Text, "메모.txt", "EUC-KR")
    });
    assert_eq!(
        labelled(&window, "첫 줄입니다", AccessibleRole::Text).len(),
        1
    );
    assert_eq!(labelled(&window, "EUC-KR", AccessibleRole::Text).len(), 1);
    assert_eq!(labelled(&window, "메모.txt", AccessibleRole::Text).len(), 1);

    // A folder's count, and a file that cannot be shown.
    window.set_preview(preview_of(PreviewKind::Folder, "문서", "항목 12개"));
    assert_eq!(
        labelled(&window, "항목 12개", AccessibleRole::Text).len(),
        1
    );
    window.set_preview(preview_of(
        PreviewKind::Unavailable,
        "깨진.png",
        "미리 볼 수 없습니다",
    ));
    assert_eq!(
        labelled(&window, "미리 볼 수 없습니다", AccessibleRole::Text).len(),
        1
    );
    assert!(labelled(&window, "깨진.png", AccessibleRole::Image).is_empty());
}

#[test]
fn the_preview_panel_never_takes_the_keys() {
    let window = window();
    let keys = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&keys);
    window.on_key(move |text, _, _, _, _| {
        record.borrow_mut().push(text.to_string());
        true
    });
    window.set_show_preview(true);
    window.set_preview(PreviewView {
        lines: lines(&["a", "b", "c"]),
        ..preview_of(PreviewKind::Text, "a.txt", "UTF-8")
    });
    // A click in the panel, on its text: the panes keep the keys.
    let panel = &labelled(&window, "미리 보기", AccessibleRole::Groupbox)[0];
    let (at, size) = (panel.absolute_position(), panel.size());
    click_at(&window, at.x + size.width / 2.0, at.y + size.height / 3.0);
    let down = char::from(Key::DownArrow).to_string();
    let tab = char::from(Key::Tab).to_string();
    chord(&window, &[&down]);
    chord(&window, &[&tab]);
    assert_eq!(keys.borrow().as_slice(), [down, tab]);
}

#[test]
fn quick_look_takes_every_key_while_it_shows() {
    let window = window();
    let keys = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&keys);
    window.on_key(move |text, _, _, _, _| {
        record.borrow_mut().push(text.to_string());
        true
    });
    let looked = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&looked);
    window.on_quick_look_key(move |text, ctrl, _, _, _| {
        record.borrow_mut().push((text.to_string(), ctrl));
    });
    let closed = Rc::new(RefCell::new(0));
    let record = Rc::clone(&closed);
    window.on_quick_look_closed(move || *record.borrow_mut() += 1);
    window.set_preview(preview_of(PreviewKind::Plain, "a.bin", ""));
    window.set_quick_look(true);
    assert_eq!(
        labelled(&window, "a.bin", AccessibleRole::Text).len(),
        1,
        "its name shows"
    );

    let control = char::from(Key::Control).to_string();
    let [down, left, right, tab, esc, delete] = [
        Key::DownArrow,
        Key::LeftArrow,
        Key::RightArrow,
        Key::Tab,
        Key::Escape,
        Key::Delete,
    ]
    .map(|key| char::from(key).to_string());
    for keys in [
        vec![left.as_str()],
        vec![right.as_str()],
        vec![down.as_str()],
        vec![tab.as_str()],
        vec![delete.as_str()],
        vec!["a"],
        vec![&control, "t"],
        vec![" "],
        vec![esc.as_str()],
    ] {
        chord(&window, &keys);
    }
    // None reached the panes; Rust had them all.
    assert_eq!(keys.borrow().as_slice(), Vec::<String>::new());
    let had: Vec<(String, bool)> = looked
        .borrow()
        .iter()
        .filter(|(text, _)| *text != control)
        .cloned()
        .collect();
    let plain = |text: &str| (text.to_owned(), false);
    assert_eq!(
        had,
        [
            plain(&left),
            plain(&right),
            plain(&down),
            plain(&tab),
            plain(&delete),
            plain("a"),
            ("t".to_owned(), true),
            plain(" "),
            plain(&esc),
        ]
    );

    // The text of a text scrolls with ↑ ↓, which stay in quick look.
    looked.borrow_mut().clear();
    window.set_preview(PreviewView {
        lines: lines(&["줄"; 200]),
        ..preview_of(PreviewKind::Text, "long.txt", "UTF-8")
    });
    chord(&window, &[&down]);
    assert_eq!(looked.borrow().as_slice(), Vec::<(String, bool)>::new());
    assert_eq!(keys.borrow().as_slice(), Vec::<String>::new());

    // A click on the card does nothing; one beside it closes.
    let card = &labelled(&window, "빠른 보기", AccessibleRole::Groupbox)[0];
    let (at, size) = (card.absolute_position(), card.size());
    click_at(&window, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert_eq!(*closed.borrow(), 0);
    click_at(&window, 4.0, 4.0);
    assert_eq!(*closed.borrow(), 1);

    // Closed, the panes have the keys again.
    window.set_quick_look(false);
    chord(&window, &[&down]);
    assert_eq!(keys.borrow().as_slice(), [down]);
}

#[test]
fn the_menus_have_the_clipboard_with_the_keys_of_the_system() {
    use slint::platform::PointerEventButton;
    let (window, _calls, _models, _tree) = drag_window();
    let commands = Calls::default();
    let record = commands.clone();
    window.on_command(move |name| record.note(name.to_string()));
    let row_menu = || {
        labelled(&window, "x.txt", AccessibleRole::ListItem)
            .remove(0)
            .mock_single_click(PointerEventButton::Right);
    };
    let blank_menu = || {
        labelled(&window, "항목 목록", AccessibleRole::List)
            .remove(0)
            .mock_single_click(PointerEventButton::Right);
    };
    let view_menu = || {
        labelled(&window, "보기", AccessibleRole::Button)
            .remove(0)
            .invoke_accessible_default_action();
    };
    let shows = |title: &str| {
        ElementHandle::find_by_accessible_label(&window, title)
            .next()
            .is_some()
    };

    // File Explorer's: cut, copy, paste, and its 경로로 복사 in quotes.
    ui_bridge::show_shortcuts(&window, Os::Windows);
    for (item, command) in [
        ("잘라내기  (Ctrl+X)", "cut"),
        ("복사  (Ctrl+C)", "copy-files"),
        ("붙여넣기  (Ctrl+V)", "paste-into"),
    ] {
        row_menu();
        choose_item(&window, item);
        assert_eq!(commands.take(), [command], "{item}");
    }
    row_menu();
    assert!(!shows("여기로 이동"), "Finder's move alone");
    choose_item(&window, "경로 복사");
    for title in ["전체 경로", "이름만", "UNC 경로", "슬래시 형식", "파일 URL"] {
        assert!(shows(title), "{title}");
    }
    choose_item(&window, "따옴표 포함  (Ctrl+Shift+C)");
    assert_eq!(commands.take(), ["copy-path-2"]);
    // The part without items pastes into the pane's folder.
    blank_menu();
    choose_item(&window, "붙여넣기  (Ctrl+V)");
    assert_eq!(commands.take(), ["paste"]);
    // The 보기 menu has 경로 복사 too, for the keyboard (F10).
    view_menu();
    choose_item(&window, "경로 복사");
    choose_item(&window, "파일 URL");
    assert_eq!(commands.take(), ["copy-path-5"]);

    // Finder's: no cut of files, ⌥⌘V moves here, and its 경로 이름 복사 as it is.
    ui_bridge::show_shortcuts(&window, Os::MacOs);
    row_menu();
    assert!(!shows("잘라내기"), "Finder cuts no files");
    choose_item(&window, "여기로 이동  (⌥⌘V)");
    assert_eq!(commands.take(), ["paste-move-into"]);
    row_menu();
    choose_item(&window, "복사  (⌘C)");
    assert_eq!(commands.take(), ["copy-files"]);
    row_menu();
    choose_item(&window, "경로 복사");
    choose_item(&window, "전체 경로  (⌥⌘C)");
    assert_eq!(commands.take(), ["copy-path-0"]);
    blank_menu();
    choose_item(&window, "여기로 이동  (⌥⌘V)");
    assert_eq!(commands.take(), ["paste-move"]);
}

#[test]
fn a_cut_item_tells_screen_readers_in_every_view() {
    let (window, _calls, models, _tree) = drag_window();
    let described = |name: &str| {
        labelled(&window, name, AccessibleRole::ListItem)
            .remove(0)
            .accessible_description()
            .map(|text| text.to_string())
    };
    models[0].set_cut(std::collections::HashSet::from([std::ffi::OsString::from(
        "x.txt",
    )]));
    for mode in [ViewMode::Details, ViewMode::List, ViewMode::LargeIcons] {
        left_in(&window, &models, mode);
        assert_eq!(
            described("x.txt").as_deref(),
            Some("잘라낸 항목"),
            "{mode:?}"
        );
        assert_eq!(described("docs").as_deref(), Some(""), "{mode:?}");
    }
    models[0].set_cut(std::collections::HashSet::new());
    left_in(&window, &models, ViewMode::Details);
    assert_eq!(described("x.txt").as_deref(), Some(""));
}

#[test]
fn the_text_fields_keep_their_own_clipboard_keys() {
    let window = window();
    let keys = Calls::default();
    let record = keys.clone();
    let control = special(Key::Control);
    let shift = special(Key::Shift);
    let modifier = [control.clone(), shift.clone()];
    window.on_key(move |text, ctrl, shift, _, _| {
        if !modifier.contains(&text.to_string()) {
            record.note(format!("{text} {ctrl} {shift}"));
        }
        true
    });
    let clipboard_keys = |window: &MainWindow| {
        for key in ["c", "x", "v"] {
            chord(window, &[&control, key]);
        }
        chord(window, &[&control, &shift, "C"]);
    };
    // The panes take them as shortcuts.
    clipboard_keys(&window);
    assert_eq!(
        keys.take(),
        [
            "c true false",
            "x true false",
            "v true false",
            "C true true"
        ]
    );
    // The filter box copies, cuts and pastes its text itself.
    let field = labelled(&window, "이 폴더에서 거르기", AccessibleRole::TextInput).remove(0);
    field.mock_single_click(slint::platform::PointerEventButton::Left);
    clipboard_keys(&window);
    assert_eq!(keys.take(), Vec::<String>::new());
    press(&window, Key::Escape);
    // So does the path field, while a syllable is composed too.
    let mut view = window.get_left_pane();
    view.path_text = "/start".into();
    window.set_left_pane(view);
    window.invoke_edit_path(0);
    clipboard_keys(&window);
    compose(&window, "ㄱ");
    clipboard_keys(&window);
    assert_eq!(keys.take(), Vec::<String>::new());
}
