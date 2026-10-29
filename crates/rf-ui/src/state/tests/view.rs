//! The view modes, the columns, the orders and the views remembered per folder (module
//! `state::view`), as the controller drives them: every change goes through
//! [`AppState::settle_views`] before its effects are carried out.

use super::*;
use crate::pane::INLINE_SORT_ROWS;

/// `state` after a change that gave `effects`, as the controller leaves it: the tabs settled.
fn settled(state: &mut AppState, mut effects: Vec<Effect>) -> Vec<Effect> {
    state.settle_views(&mut effects);
    effects
}

/// A window state like [`ready`], settled: its tabs show the views remembered (none yet).
fn ready_settled(left: Vec<Entry>, right: Vec<Entry>) -> AppState {
    let mut state = ready(left, right);
    assert_eq!(settled(&mut state, Vec::new()), []);
    state
}

fn mode(state: &AppState, side: usize) -> ViewMode {
    state.panes()[side].active_tab().view().mode
}

fn files(count: usize) -> Vec<Entry> {
    (0..count).map(|i| file(&format!("f{i:02}"))).collect()
}

#[test]
fn a_folder_remembers_its_view_and_shows_it_when_a_tab_comes_back() {
    let mut state = ready_settled(vec![dir("sub")], vec![file("z")]);
    let effects = state.set_view_mode(LEFT, ViewMode::LargeIcons);
    assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 0 }]);
    assert_eq!(mode(&state, LEFT), ViewMode::LargeIcons);
    assert_eq!(
        state.views().own(&folder("a")).map(|own| own.view.mode),
        Some(ViewMode::LargeIcons)
    );
    // The right pane's folder is sorted by size, which it remembers too.
    state.sort_by(RIGHT, SortKey::Size, false);
    assert_eq!(
        state
            .views()
            .own(&folder("b"))
            .map(|own| own.view.sort.primary.key),
        Some(SortKey::Size)
    );

    // The left pane goes to b: the listing is asked for in b's order, and shows in b's view.
    assert_eq!(
        state.active(),
        RIGHT,
        "the pane whose view changed has the keyboard"
    );
    state.activate(LEFT);
    let effects = state.go_to(folder("b"));
    let effects = settled(&mut state, effects);
    assert_eq!(the_load(&effects).sort.primary.key, SortKey::Size);
    assert_eq!(mode(&state, LEFT), ViewMode::Details);
    assert_eq!(
        state.panes()[LEFT].active_tab().sort().primary.key,
        SortKey::Size
    );

    // Back at a: its icons again.
    let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("used");
    let effects = settled(&mut state, effects);
    assert_eq!(the_load(&effects).sort.primary.key, SortKey::Name);
    assert_eq!(mode(&state, LEFT), ViewMode::LargeIcons);

    // A new tab at a folder shows that folder's view.
    let effects = press(&mut state, "t", CTRL).expect("used");
    settled(&mut state, effects);
    assert_eq!(mode(&state, LEFT), ViewMode::LargeIcons);
}

#[test]
fn the_view_keys_and_the_menu_change_the_mode_and_the_columns() {
    let mut state = ready_settled(files(3), vec![]);
    for (text, mods, wanted) in [
        ("@", CTRL_SHIFT, ViewMode::LargeIcons),
        ("#", CTRL_SHIFT, ViewMode::MediumIcons),
        ("$", CTRL_SHIFT, ViewMode::SmallIcons),
        ("%", CTRL_SHIFT, ViewMode::List),
        ("^", CTRL_SHIFT, ViewMode::Details),
    ] {
        let effects = press(&mut state, text, mods).expect("a view key");
        assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 0 }]);
        assert_eq!(mode(&state, LEFT), wanted, "{text}");
    }
    // macOS: ⌘1 icons, ⌘2 자세히, ⌘3 목록.
    for (text, wanted) in [
        ("1", ViewMode::MediumIcons),
        ("3", ViewMode::List),
        ("2", ViewMode::Details),
    ] {
        let command = keys::command_for(text, CTRL, Os::MacOs).expect("a view key");
        state.key(command, CTRL, Paging::rows(10), Instant::now());
        assert_eq!(mode(&state, LEFT), wanted, "⌘{text}");
    }

    // The columns: shown, hidden, moved, made wider; the folder remembers them.
    state.toggle_column(LEFT, Column::Created);
    state.toggle_column(LEFT, Column::Kind);
    state.move_column(LEFT, 2, 0);
    state.set_column_width(LEFT, Column::Size, 90);
    let columns: Vec<(Column, u32)> = state.panes()[LEFT]
        .active_tab()
        .view()
        .columns
        .iter()
        .map(|shown| (shown.column, shown.width))
        .collect();
    assert_eq!(
        columns,
        [
            (Column::Created, 126),
            (Column::Size, 90),
            (Column::Modified, 126)
        ]
    );
    assert_eq!(
        state
            .views()
            .own(&folder("a"))
            .map(|own| own.view.columns.len()),
        Some(3)
    );

    // Folders mixed in, a second key from the menu.
    state.toggle_folders_first(LEFT);
    state.sort_then(LEFT, Some(SortKey::Modified));
    let sort = state.panes()[LEFT].active_tab().sort();
    assert!(!sort.folders_first);
    assert_eq!(sort.secondary.map(|by| by.key), Some(SortKey::Modified));
}

#[test]
fn the_keys_move_as_the_view_lays_the_rows_out() {
    let mut state = ready_settled(files(10), vec![]);
    // Four icons a row, two rows a page.
    let grid = Paging { line: 4, page: 2 };
    let key = |state: &mut AppState, k: Key, mods: Mods| {
        let command = keys::command_for(&special(k), mods, Os::Windows).expect("a key");
        state.key(command, mods, grid, Instant::now())
    };
    // 자세히: ← and → do nothing, and nothing changes.
    assert_eq!(key(&mut state, Key::RightArrow, NONE), Some(vec![]));
    assert_eq!(cursor(&state, LEFT), Some(0));
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());

    state.set_view_mode(LEFT, ViewMode::MediumIcons);
    assert_eq!(
        key(&mut state, Key::RightArrow, NONE),
        Some(vec![Effect::ScrollTo { side: LEFT, row: 1 }])
    );
    key(&mut state, Key::DownArrow, NONE);
    assert_eq!(
        (cursor(&state, LEFT), selected(&state, LEFT)),
        (Some(5), vec![5])
    );
    // Shift extends from the anchor; Ctrl moves the cursor alone.
    key(&mut state, Key::LeftArrow, SHIFT);
    assert_eq!(selected(&state, LEFT), [4, 5]);
    key(&mut state, Key::DownArrow, CTRL);
    assert_eq!(
        (cursor(&state, LEFT), selected(&state, LEFT)),
        (Some(8), vec![4, 5])
    );
    key(&mut state, Key::PageUp, NONE);
    assert_eq!(cursor(&state, LEFT), Some(0));

    // 목록: ↓ goes down a column, → a column across (four a column here).
    state.set_view_mode(LEFT, ViewMode::List);
    key(&mut state, Key::DownArrow, NONE);
    key(&mut state, Key::RightArrow, NONE);
    assert_eq!(cursor(&state, LEFT), Some(5));
    key(&mut state, Key::End, NONE);
    key(&mut state, Key::LeftArrow, NONE);
    assert_eq!(cursor(&state, LEFT), Some(5));
}

#[test]
fn a_file_the_cursor_reached_alone_is_not_what_commands_act_on() {
    let mut state = ready_settled(vec![file("a.txt"), file("b.txt")], vec![]);
    state.set_view_mode(LEFT, ViewMode::SmallIcons);
    let grid = Paging { line: 3, page: 1 };
    let right = keys::command_for(&special(Key::RightArrow), CTRL, Os::Windows).expect("a key");
    state.key(right, CTRL, grid, Instant::now());
    assert_eq!(cursor(&state, LEFT), Some(1));
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    // Enter launches nothing, and F2 renames nothing: the row shows unselected.
    assert_eq!(press(&mut state, &special(Key::Return), NONE), Some(vec![]));
    assert_eq!(press(&mut state, &special(Key::F2), NONE), Some(vec![]));
    assert_eq!(state.modal(), None);
    // Ctrl+Space selects it; then Enter opens it.
    press(&mut state, &special(Key::Space), CTRL);
    let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
    assert!(
        matches!(effects.as_slice(), [Effect::OpenFile(_)]),
        "{effects:?}"
    );
}

#[test]
fn a_long_folder_is_sorted_on_a_worker_and_a_stale_sort_is_dropped() {
    let mut state = ready_settled(files(INLINE_SORT_ROWS + 1), vec![]);
    let effects = state.sort_by(LEFT, SortKey::Name, false);
    // The header flips at once; the rows wait for the worker.
    let effects = settled(&mut state, effects);
    let [Effect::Sort(request)] = effects.as_slice() else {
        panic!("one sort on a worker: {effects:?}");
    };
    assert_eq!(rows(&state, LEFT)[0], "f00");
    let sorted = request.clone().run();
    assert_eq!(state.sorted(sorted), []);
    assert_eq!(rows(&state, LEFT)[0], format!("f{INLINE_SORT_ROWS}"));

    // The pane moves on before a sort comes back: it is dropped.
    let effects = state.sort_by(LEFT, SortKey::Name, false);
    let effects = settled(&mut state, effects);
    let [Effect::Sort(request)] = effects.as_slice() else {
        panic!("one sort on a worker: {effects:?}");
    };
    let late = request.clone().run();
    let effects = state.go_to(folder("b"));
    settled(&mut state, effects);
    assert_eq!(state.sorted(late), []);
    assert_eq!(rows(&state, LEFT), Vec::<&str>::new());
}

#[test]
fn a_view_applied_to_subfolders_shows_below_until_the_folder_is_reset() {
    let mut state = ready_settled(vec![dir("sub")], vec![]);
    state.set_view_mode(LEFT, ViewMode::SmallIcons);
    let effects = state.apply_view_to_subfolders(LEFT);
    assert_eq!(
        effects,
        [Effect::Notice(
            "‘a’ 폴더와 그 하위 폴더를 이 보기로 표시합니다".to_owned()
        )]
    );
    // Into a folder below: it shows the same view.
    let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
    settled(&mut state, effects);
    assert_eq!(
        state.panes()[LEFT].active_tab().location(),
        &folder("a").join("sub").expect("a name")
    );
    assert_eq!(mode(&state, LEFT), ViewMode::SmallIcons);
    // Its own view, then reset: back to what it takes from above.
    state.set_view_mode(LEFT, ViewMode::List);
    let effects = state.reset_view(LEFT);
    assert_eq!(
        effects,
        [Effect::Notice(
            "‘sub’ 폴더의 보기를 되돌렸습니다".to_owned()
        )]
    );
    assert_eq!(mode(&state, LEFT), ViewMode::SmallIcons);
    // Up, and reset there too: the default view, here and below.
    let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("used");
    settled(&mut state, effects);
    state.reset_view(LEFT);
    assert_eq!(mode(&state, LEFT), ViewMode::Details);
    assert!(state.views().is_empty());
}

#[test]
fn a_change_in_one_tab_shows_in_every_tab_at_its_folder_and_keeps_theirs() {
    let mut state = ready_settled(files(3), vec![]);
    // A second tab at a, and the right pane at a too.
    let effects = press(&mut state, "t", CTRL).expect("used");
    settled(&mut state, effects);
    state.activate(RIGHT);
    let effects = state.go_to(folder("a"));
    settled(&mut state, effects);
    let views = |state: &AppState| -> Vec<FolderView> {
        state
            .panes()
            .iter()
            .flat_map(Pane::tabs)
            .map(|tab| tab.view().clone())
            .collect()
    };
    assert_eq!(views(&state).len(), 3);

    // The new tab sorts by size: the tab behind it and the right pane show it too.
    let effects = state.sort_by(LEFT, SortKey::Size, false);
    settled(&mut state, effects);
    assert!(
        views(&state)
            .iter()
            .all(|view| view.sort.primary.key == SortKey::Size)
    );
    // The right pane hides a column, which goes on top of that order: the order stays, in
    // every tab and in what the folder remembers.
    let effects = state.toggle_column(RIGHT, Column::Kind);
    settled(&mut state, effects);
    let kept = state
        .views()
        .own(&folder("a"))
        .expect("a's own")
        .view
        .clone();
    assert_eq!(kept.sort.primary.key, SortKey::Size);
    assert!(!kept.shows(Column::Kind));
    assert!(views(&state).iter().all(|view| *view == kept));
    // The rows of every tab are in that order.
    for side in [LEFT, RIGHT] {
        assert_eq!(
            state.panes()[side].active_tab().sort().primary.key,
            SortKey::Size
        );
    }
}

#[test]
fn applying_a_view_below_asks_first_when_folders_below_have_their_own() {
    let mut state = ready_settled(vec![dir("sub")], vec![]);
    // Nothing below has a view of its own: no question.
    let effects = state.apply_view_to_subfolders(LEFT);
    assert!(
        matches!(effects.as_slice(), [Effect::Notice(_)]),
        "{effects:?}"
    );
    assert!(state.modal().is_none());

    // sub gets a view of its own; back in a, 하위 폴더에 적용 asks, and says how many.
    let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
    settled(&mut state, effects);
    state.set_view_mode(LEFT, ViewMode::List);
    let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("used");
    settled(&mut state, effects);
    state.set_view_mode(LEFT, ViewMode::LargeIcons);
    assert_eq!(state.apply_view_to_subfolders(LEFT), []);
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("a question: {:?}", state.modal());
    };
    assert_eq!(confirm.title, "하위 폴더에 적용");
    assert!(
        confirm.message.contains("하위 폴더 1개"),
        "{}",
        confirm.message
    );
    assert_eq!(confirm.ok, "적용");
    let sub = folder("a").join("sub").expect("a name");
    assert!(state.views().own(&sub).is_some(), "kept until the answer");

    // Cancelled: nothing changes. Accepted: sub forgets its own and shows a's.
    state.dialog_cancelled();
    assert!(state.views().own(&sub).is_some());
    state.apply_view_to_subfolders(LEFT);
    let effects = state.confirm_accepted();
    assert_eq!(
        effects,
        [Effect::Notice(
            "‘a’ 폴더와 그 하위 폴더를 이 보기로 표시합니다".to_owned()
        )]
    );
    assert!(state.modal().is_none());
    assert!(state.views().own(&sub).is_none());
    assert_eq!(
        state.views().clone().view_for(&sub).mode,
        ViewMode::LargeIcons
    );
}
