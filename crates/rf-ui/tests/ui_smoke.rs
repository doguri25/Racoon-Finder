//! Headless checks of the Slint screens (no window system needed).

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
use rf_core::{Entry, EntryKind, VPath};
use rf_ui::loader::LoadEvent;
use rf_ui::pane::Pane;
use rf_ui::selection::Selection;
use rf_ui::ui_bridge::{FileRows, PaneModels};
use rf_ui::{MainWindow, PaneView, PlaceRow, Theme};
use rf_vfs::VisibilityFilter;
use slint::platform::{Key, WindowEvent};
use slint::{Color, ComponentHandle, LogicalSize, Model, ModelRc, VecModel};

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
fn places_show_in_the_sidebar() {
    let window = window();
    let clicked = Rc::new(RefCell::new(None));
    let record = Rc::clone(&clicked);
    window.on_place_clicked(move |i| *record.borrow_mut() = Some(i));
    window.set_places(ModelRc::new(VecModel::from(vec![
        PlaceRow {
            label: "홈".into(),
            ..PlaceRow::default()
        },
        PlaceRow {
            label: "시스템 디스크".into(),
            info: "128 GB 남음 / 512 GB".into(),
            fill: 0.75,
            is_drive: true,
        },
    ])));

    assert_eq!(labelled(&window, "즐겨찾기", AccessibleRole::Text).len(), 1);
    assert_eq!(labelled(&window, "이 PC", AccessibleRole::Text).len(), 1);
    let drive = labelled(&window, "시스템 디스크", AccessibleRole::Button);
    assert_eq!(
        drive[0].accessible_description().as_deref(),
        Some("128 GB 남음 / 512 GB")
    );
    drive[0].invoke_accessible_default_action();
    assert_eq!(*clicked.borrow(), Some(1));
}
