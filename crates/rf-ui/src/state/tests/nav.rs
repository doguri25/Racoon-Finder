//! The panes, the keys, the tree and the start folders.

use super::*;

#[test]
fn startup_lists_both_panes() {
    let state = ready(vec![dir("x"), file("y")], vec![file("z")]);
    assert_eq!(state.active(), LEFT);
    assert_eq!(rows(&state, LEFT), ["x", "y"]);
    assert_eq!(rows(&state, RIGHT), ["z"]);
    assert_eq!(state.generations().len(), 2);
}

#[test]
fn tab_switches_panes_and_clicks_activate() {
    let mut state = ready(vec![file("x")], vec![file("z")]);
    assert_eq!(press(&mut state, &special(Key::Tab), NONE), Some(vec![]));
    assert_eq!(state.active(), RIGHT);
    press(&mut state, &special(Key::Tab), SHIFT);
    assert_eq!(state.active(), LEFT);
    state.click_row(RIGHT, 0, false, false);
    assert_eq!(state.active(), RIGHT);
    assert_eq!(selected(&state, RIGHT), [0]);
}

#[test]
fn ctrl_tab_goes_round_the_tabs_of_the_active_pane() {
    let mut state = ready(vec![file("x")], vec![file("z")]);
    press(&mut state, "t", CTRL);
    press(&mut state, "t", CTRL);
    let shown = |state: &AppState| state.panes()[LEFT].active();
    assert_eq!(shown(&state), 2);
    // After the last tab the first, and back.
    assert_eq!(press(&mut state, &special(Key::Tab), CTRL), Some(vec![]));
    assert_eq!(shown(&state), 0);
    press(&mut state, &special(Key::Tab), CTRL);
    assert_eq!(shown(&state), 1);
    press(&mut state, &special(Key::Tab), CTRL_SHIFT);
    press(&mut state, &special(Key::Tab), CTRL_SHIFT);
    assert_eq!(shown(&state), 2);
    // The other pane, with one tab, keeps it.
    press(&mut state, &special(Key::Tab), NONE);
    press(&mut state, &special(Key::Tab), CTRL);
    assert_eq!(state.panes()[RIGHT].active(), 0);
    assert_eq!(shown(&state), 2);
}

#[test]
fn the_filter_box_and_the_view_menu_take_the_keys() {
    let mut state = ready(vec![file("x")], vec![]);
    for text in ["f", "e"] {
        assert_eq!(
            press(&mut state, text, CTRL),
            Some(vec![Effect::FocusFilter])
        );
    }
    assert_eq!(
        press(&mut state, &special(Key::F3), NONE),
        Some(vec![Effect::FocusFilter])
    );
    assert_eq!(
        press(&mut state, &special(Key::F10), NONE),
        Some(vec![Effect::ShowViewMenu])
    );
}

#[test]
fn keys_of_commands_still_to_come_go_on_to_the_system() {
    let mut state = ready(vec![file("x")], vec![file("z")]);
    for (text, mods) in [
        (special(Key::F5), NONE),
        ("c".to_owned(), CTRL),
        ("v".to_owned(), CTRL),
        ("x".to_owned(), CTRL),
        (special(Key::Return), ALT),
        (special(Key::F10), SHIFT),
    ] {
        assert_eq!(press(&mut state, &text, mods), None, "{text:?} {mods:?}");
    }
    // Nothing changed meanwhile.
    assert_eq!(state.active(), LEFT);
    assert_eq!(state.modal(), None);
}

#[test]
fn keys_move_the_cursor_and_select() {
    let names = ["a", "b", "c", "d", "e"];
    let mut state = ready(names.iter().copied().map(file).collect(), vec![]);
    let down = special(Key::DownArrow);
    // A folder just listed has the cursor on its first row, nothing selected.
    assert_eq!(
        (cursor(&state, LEFT), selected(&state, LEFT)),
        (Some(0), vec![])
    );
    assert_eq!(
        press(&mut state, &down, NONE),
        Some(vec![Effect::ScrollTo { side: LEFT, row: 1 }])
    );
    assert_eq!(
        (cursor(&state, LEFT), selected(&state, LEFT)),
        (Some(1), vec![1])
    );
    // Shift extends from the anchor.
    press(&mut state, &down, SHIFT);
    press(&mut state, &down, SHIFT);
    assert_eq!(selected(&state, LEFT), [1, 2, 3]);
    // Ctrl+Space flips the cursor's row; Esc clears; Ctrl+A selects all.
    press(&mut state, &special(Key::Space), CTRL);
    assert_eq!(selected(&state, LEFT), [1, 2]);
    press(&mut state, &special(Key::Escape), NONE);
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    assert_eq!(cursor(&state, LEFT), Some(3));
    press(&mut state, "a", CTRL);
    assert_eq!(selected(&state, LEFT), [0, 1, 2, 3, 4]);
    // Home, End and the page keys stop at the ends.
    press(&mut state, &special(Key::Home), NONE);
    assert_eq!(cursor(&state, LEFT), Some(0));
    assert_eq!(
        press(&mut state, &special(Key::PageDown), NONE),
        Some(vec![Effect::ScrollTo { side: LEFT, row: 4 }])
    );
    press(&mut state, &special(Key::End), SHIFT);
    assert_eq!(selected(&state, LEFT), [4]);
    // Clicks: Ctrl adds, Shift takes the range from the anchor.
    state.click_row(LEFT, 1, false, false);
    state.click_row(LEFT, 3, true, false);
    assert_eq!(selected(&state, LEFT), [1, 3]);
    state.click_row(LEFT, 4, false, true);
    assert_eq!(selected(&state, LEFT), [3, 4]);
}

#[test]
fn typing_jumps_to_a_name() {
    let mut state = ready(vec![file("apple"), file("banana"), file("berry")], vec![]);
    let now = Instant::now();
    let effects = state.key(
        Command::TypeToSelect("be".into()),
        NONE,
        Paging::rows(10),
        now,
    );
    assert_eq!(effects, Some(vec![Effect::ScrollTo { side: LEFT, row: 2 }]));
    assert_eq!(selected(&state, LEFT), [2]);
    // No match moves nothing, and the key is still used.
    let effects = state.key(
        Command::TypeToSelect("x".into()),
        NONE,
        Paging::rows(10),
        now + Duration::from_secs(5),
    );
    assert_eq!(effects, Some(vec![]));
    assert_eq!(selected(&state, LEFT), [2]);
}

#[test]
fn open_follows_the_row_kind() {
    let link = Entry::new("latest", EntryKind::Symlink);
    let mut state = ready(vec![dir("sub"), file("notes.txt"), link], vec![]);
    let generation = state.panes()[LEFT].active_tab().generation();

    assert_eq!(
        state.open_row(LEFT, 1),
        [Effect::OpenFile(
            folder("a").join("notes.txt").expect("a name")
        )]
    );
    assert_eq!(
        state.open_row(LEFT, 2),
        [Effect::Follow {
            generation,
            link: folder("a").join("latest").expect("a name"),
        }]
    );
    // Enter opens the cursor's row: the folder shows in the tab.
    state.click_row(LEFT, 0, false, false);
    let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
    let request = the_load(&effects);
    assert_eq!(request.path, folder("a").join("sub").expect("a name"));
    assert_ne!(request.generation, generation);
    // Back goes to `a` again, and Up to its parent.
    let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("used");
    assert_eq!(the_load(&effects).path, folder("a"));
    let effects = press(&mut state, &special(Key::UpArrow), ALT).expect("used");
    let temp = VPath::local(std::env::temp_dir()).expect("a local path");
    assert_eq!(the_load(&effects).path, temp);
}

#[test]
fn backspace_goes_back_through_the_history_as_in_explorer() {
    let mut state = ready(vec![dir("sub")], vec![]);
    let sub = folder("a").join("sub").expect("a name");
    assert_eq!(the_load(&state.open_row(LEFT, 0)).path, sub);
    assert_eq!(the_load(&state.go_to(folder("b"))).path, folder("b"));
    // Backspace is Back on Windows: the folder before `b`, not its parent; then the one
    // before that.
    let effects = press(&mut state, &special(Key::Backspace), NONE).expect("used");
    assert_eq!(the_load(&effects).path, sub);
    let effects = press(&mut state, &special(Key::Backspace), NONE).expect("used");
    assert_eq!(the_load(&effects).path, folder("a"));
    // And Forward comes back.
    let effects = press(&mut state, &special(Key::RightArrow), ALT).expect("used");
    assert_eq!(the_load(&effects).path, sub);
}

#[test]
fn enter_launches_only_a_row_the_user_chose() {
    // A folder just opened has the cursor on its first row for the keys to start from. Enter
    // goes into a folder there (that only moves the tab).
    let mut state = ready(vec![dir("sub"), file("notes.txt")], vec![]);
    let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
    let request = the_load(&effects);
    assert_eq!(request.path, folder("a").join("sub").expect("a name"));
    // The folder's first row is a file nobody chose: Enter launches nothing.
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![file("x.txt"), file("y.txt")],
    });
    assert_eq!(cursor(&state, LEFT), Some(0));
    assert_eq!(press(&mut state, &special(Key::Return), NONE), Some(vec![]));
    // Down and back up chooses it: Enter opens it.
    press(&mut state, &special(Key::DownArrow), NONE);
    press(&mut state, &special(Key::UpArrow), NONE);
    let x = request.path.join("x.txt").expect("a name");
    assert_eq!(
        press(&mut state, &special(Key::Return), NONE),
        Some(vec![Effect::OpenFile(x)])
    );
    // Once Esc has taken the selection away, the row is not chosen any more.
    press(&mut state, &special(Key::Escape), NONE);
    assert_eq!(press(&mut state, &special(Key::Return), NONE), Some(vec![]));

    // A link may lead to a file: Enter follows it only once the user chose its row.
    let mut state = ready(vec![Entry::new("latest", EntryKind::Symlink)], vec![]);
    assert_eq!(press(&mut state, &special(Key::Return), NONE), Some(vec![]));
    press(&mut state, &special(Key::DownArrow), NONE);
    let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
    assert!(
        matches!(effects.as_slice(), [Effect::Follow { .. }]),
        "{effects:?}"
    );
}

#[test]
fn a_late_link_result_is_dropped() {
    let mut state = ready(vec![Entry::new("latest", EntryKind::Symlink)], vec![]);
    let effects = state.open_row(LEFT, 0);
    let Effect::Follow { generation, link } = effects[0].clone() else {
        panic!("a link is followed: {effects:?}");
    };
    // The user moves on before the link's target is known.
    state.go_to(folder("elsewhere"));
    let late = state.followed(Followed {
        generation,
        result: Ok(OpenAction::Navigate(link.clone())),
    });
    assert_eq!(late, []);
    assert_eq!(
        state.panes()[LEFT].active_tab().location(),
        &folder("elsewhere")
    );

    // In time, a link to a folder shows it, one to a file opens it, and a broken one
    // gives a notice.
    let generation = state.panes()[LEFT].active_tab().generation();
    assert_eq!(
        state.followed(Followed {
            generation,
            result: Ok(OpenAction::OpenFile(link.clone())),
        }),
        [Effect::OpenFile(link.clone())]
    );
    assert_eq!(
        state.followed(Followed {
            generation,
            result: Err(ErrorKind::NotFound),
        }),
        [Effect::Notice(
            "열 수 없습니다: 찾을 수 없습니다".to_owned()
        )]
    );
    let effects = state.followed(Followed {
        generation,
        result: Ok(OpenAction::Navigate(link.clone())),
    });
    assert_eq!(the_load(&effects).path, link);
}

#[test]
fn a_vanished_folder_goes_to_the_nearest_parent() {
    let mut state = ready(vec![file("x")], vec![]);
    let generation = state.panes()[LEFT].active_tab().generation();
    let effects = state.loaded(LoadEvent::Vanished { generation });
    assert_eq!(
        effects,
        [Effect::FindExisting {
            generation,
            path: folder("a"),
        }]
    );
    // The ticket stays until the tab moves on: a listing still running still comes.
    assert!(state.generations().contains(&generation));

    let parent = folder("a").parent().expect("a parent");
    let effects = state.found_existing(
        generation,
        &folder("a"),
        Some(parent.clone()),
        Instant::now(),
    );
    assert_eq!(the_load(&effects).path, parent);
    // The user is told why the pane moved.
    assert!(
        effects.contains(&Effect::Notice(
            "‘a’ 폴더가 없어져 상위 폴더로 옮겼습니다".to_owned()
        )),
        "{effects:?}"
    );
    // A second answer for the same generation is late.
    assert_eq!(
        state.found_existing(generation, &folder("a"), Some(parent), Instant::now()),
        []
    );

    // A tab that does not show moves without a word: its pane shows another one.
    let effects = state.add_tab(LEFT);
    let shown = the_load(&effects).generation;
    state.loaded(LoadEvent::Listed {
        generation: shown,
        entries: Vec::new(),
    });
    state.select_tab(LEFT, 0);
    let behind = state.panes()[LEFT].tabs()[1].generation();
    let effects = state.found_existing(behind, &folder("x"), Some(folder("y")), Instant::now());
    assert_eq!(the_load(&effects).path, folder("y"));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Notice(_))),
        "{effects:?}"
    );
}

#[test]
fn a_replaced_folder_is_listed_again_but_not_in_a_loop() {
    let mut state = ready(vec![file("old")], vec![]);
    let now = Instant::now();
    let generation = state.panes()[LEFT].active_tab().generation();
    state.loaded(LoadEvent::Vanished { generation });
    let effects = state.found_existing(generation, &folder("a"), Some(folder("a")), now);
    let request = the_load(&effects);
    assert_eq!(request.path, folder("a"));
    // A reload keeps the rows until the new listing comes.
    assert_eq!(rows(&state, LEFT), ["old"]);

    // The new watch ends at once again: no second reload within the gap, but one when it
    // has passed.
    state.loaded(LoadEvent::Vanished {
        generation: request.generation,
    });
    let soon = now + Duration::from_secs(1);
    assert_eq!(
        state.found_existing(request.generation, &folder("a"), Some(folder("a")), soon),
        [Effect::ReloadLater {
            generation: request.generation,
            path: folder("a"),
            after: AUTO_RELOAD_GAP.saturating_sub(Duration::from_secs(1)),
        }]
    );
    // An answer after the gap reloads at once, and the waiting reload then has nothing to do.
    let later = now + AUTO_RELOAD_GAP + Duration::from_secs(1);
    let effects = state.found_existing(request.generation, &folder("a"), Some(folder("a")), later);
    assert_eq!(the_load(&effects).path, folder("a"));
    assert_eq!(
        state.reload_due(request.generation, &folder("a"), later),
        []
    );
}

#[test]
fn a_folder_replaced_twice_quickly_is_listed_once_more_after_the_gap() {
    let mut state = ready(vec![file("first")], vec![]);
    let now = Instant::now();
    let a = folder("a");
    // The first replacement reloads at once.
    let generation = state.panes()[LEFT].active_tab().generation();
    state.loaded(LoadEvent::Vanished { generation });
    let first = the_load(&state.found_existing(generation, &a, Some(a.clone()), now));

    // The second, a second later, waits for the rest of the gap; asking again adds nothing.
    state.loaded(LoadEvent::Vanished {
        generation: first.generation,
    });
    let soon = now + Duration::from_secs(1);
    let later = vec![Effect::ReloadLater {
        generation: first.generation,
        path: a.clone(),
        after: AUTO_RELOAD_GAP.saturating_sub(Duration::from_secs(1)),
    }];
    assert_eq!(
        state.found_existing(first.generation, &a, Some(a.clone()), soon),
        later
    );
    assert_eq!(
        state.found_existing(first.generation, &a, Some(a.clone()), soon),
        []
    );

    // When the gap has passed, exactly one more reload comes.
    let due = now + AUTO_RELOAD_GAP;
    let second = the_load(&state.reload_due(first.generation, &a, due));
    assert_eq!(second.path, a);
    assert_ne!(second.generation, first.generation);
    assert_eq!(state.reload_due(first.generation, &a, due), []);
    assert_eq!(
        state.panes()[LEFT].active_tab().generation(),
        second.generation
    );
}

#[test]
fn a_waiting_reload_of_a_tab_that_moved_on_does_nothing() {
    let mut state = ready(vec![file("x")], vec![]);
    let now = Instant::now();
    let a = folder("a");
    let generation = state.panes()[LEFT].active_tab().generation();
    let first = the_load(&state.found_existing(generation, &a, Some(a.clone()), now));
    let soon = now + Duration::from_secs(1);
    let effects = state.found_existing(first.generation, &a, Some(a.clone()), soon);
    assert!(matches!(effects.as_slice(), [Effect::ReloadLater { .. }]));
    // The user opens another folder before the gap has passed.
    state.go_to(folder("elsewhere"));
    assert_eq!(
        state.reload_due(first.generation, &a, now + AUTO_RELOAD_GAP),
        []
    );
    assert_eq!(
        state.panes()[LEFT].active_tab().location(),
        &folder("elsewhere")
    );
}

#[test]
fn a_folder_with_nothing_left_goes_home() {
    let mut state = ready(vec![], vec![]);
    let generation = state.panes()[RIGHT].active_tab().generation();
    let effects = state.found_existing(generation, &folder("b"), None, Instant::now());
    assert_eq!(the_load(&effects).path, folder("home"));
    assert!(
        effects.contains(&Effect::Notice(
            "‘b’ 폴더가 없어져 홈 폴더로 옮겼습니다".to_owned()
        )),
        "{effects:?}"
    );
    assert_eq!(
        state.panes()[RIGHT].active_tab().location(),
        &folder("home")
    );
}

#[test]
fn hidden_items_toggle_in_every_tab() {
    let mut dot = file(".dot");
    dot.attributes |= Attributes::HIDDEN;
    let mut state = ready(vec![dot.clone(), file("x")], vec![dot, file("y")]);
    assert_eq!(rows(&state, LEFT), ["x"]);
    assert_eq!(press(&mut state, "h", CTRL), Some(vec![]));
    assert!(state.show_hidden());
    assert_eq!(rows(&state, LEFT), [".dot", "x"]);
    assert_eq!(rows(&state, RIGHT), [".dot", "y"]);
    // A new tab takes the setting.
    let request = the_load(&press(&mut state, "t", CTRL).expect("used"));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![{
            let mut dot = file(".new");
            dot.attributes |= Attributes::HIDDEN;
            dot
        }],
    });
    assert_eq!(rows(&state, LEFT), [".new"]);
    press(&mut state, "h", CTRL);
    assert_eq!(rows(&state, LEFT), Vec::<&str>::new());
}

#[test]
fn tabs_open_show_and_close() {
    let mut state = ready(vec![file("x")], vec![]);
    let first = state.panes()[LEFT].active_tab().generation();
    let effects = state.add_tab(LEFT);
    let request = the_load(&effects);
    assert_eq!(request.path, folder("a"));
    assert_eq!(state.panes()[LEFT].tabs().len(), 2);
    assert_eq!(state.panes()[LEFT].active(), 1);
    state.select_tab(LEFT, 0);
    assert_eq!(state.panes()[LEFT].active(), 0);
    // Ctrl+W closes the tab that shows; the last tab stays.
    press(&mut state, "w", CTRL);
    assert_eq!(state.panes()[LEFT].tabs().len(), 1);
    assert!(!state.generations().contains(&first));
    assert!(state.generations().contains(&request.generation));
    press(&mut state, "w", CTRL);
    assert_eq!(state.panes()[LEFT].tabs().len(), 1);
    // A click on the other pane's tab makes that pane active.
    state.close_tab(RIGHT, 0);
    assert_eq!(state.active(), RIGHT);
}

#[test]
fn crumbs_and_typed_paths_navigate() {
    let mut state = ready(vec![], vec![]);
    let chain = ancestors(&folder("a"));
    assert_eq!(chain.last(), Some(&folder("a")));
    assert_eq!(chain[0].parent(), None);
    let effects = state.open_crumb(LEFT, 0);
    assert_eq!(the_load(&effects).path, chain[0]);
    assert_eq!(state.open_crumb(LEFT, 99), []);

    let typed = std::env::temp_dir().join("typed");
    let effects = state.enter_path(RIGHT, &format!("  {}  ", typed.display()));
    assert_eq!(the_load(&effects).path, folder("typed"));
    assert_eq!(state.active(), RIGHT);
    assert_eq!(state.enter_path(RIGHT, "   "), []);
    assert_eq!(
        state.enter_path(RIGHT, "relative/path"),
        [Effect::Notice(
            "열 수 없습니다: 쓸 수 없는 이름이나 경로입니다".to_owned()
        )]
    );
    assert_eq!(
        press(&mut state, "l", CTRL),
        Some(vec![Effect::EditPath(RIGHT)])
    );
}

#[test]
fn sort_and_retry_act_on_the_clicked_pane() {
    // The loader sends a listing sorted by name.
    let mut state = ready(vec![file("a"), file("b")], vec![]);
    assert_eq!(rows(&state, LEFT), ["a", "b"]);
    state.sort(LEFT, SortKey::Name);
    assert_eq!(rows(&state, LEFT), ["b", "a"]);
    let sort = state.panes()[LEFT].active_tab().sort();
    assert_eq!(sort.primary.order, SortOrder::Descending);
    let effects = state.retry(RIGHT);
    assert_eq!(the_load(&effects).path, folder("b"));
    assert_eq!(state.active(), RIGHT);
}

#[test]
fn a_right_click_keeps_a_selection_it_falls_in() {
    let mut state = ready(vec![file("a"), file("b"), file("c")], vec![]);
    state.click_row(LEFT, 0, false, false);
    state.click_row(LEFT, 1, true, false);
    state.row_menu(RIGHT, 0);
    assert_eq!(state.active(), RIGHT);
    state.row_menu(LEFT, 0);
    assert_eq!(state.active(), LEFT);
    assert_eq!(selected(&state, LEFT), [0, 1]);
    assert_eq!(cursor(&state, LEFT), Some(0));
    state.row_menu(LEFT, 2);
    assert_eq!(selected(&state, LEFT), [2]);
    assert_eq!(cursor(&state, LEFT), Some(2));
}

#[test]
fn late_events_of_a_closed_tab_are_dropped() {
    let mut state = ready(vec![file("x")], vec![]);
    let request = the_load(&state.add_tab(LEFT));
    state.close_tab(LEFT, 1);
    let effects = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![file("late")],
    });
    assert_eq!(effects, []);
    assert_eq!(rows(&state, LEFT), ["x"]);
    assert_eq!(
        state.found_existing(request.generation, &folder("a"), None, Instant::now()),
        []
    );
}

pub(super) fn temp() -> VPath {
    VPath::local(std::env::temp_dir()).expect("a local path")
}

pub(super) fn drive(label: &str, path: &VPath) -> Place {
    Place {
        label: label.to_owned(),
        path: path.clone(),
        kind: PlaceKind::Drive,
        space: None,
    }
}

/// The one listing of the folder tree among `effects`.
pub(super) fn the_tree_load(effects: &[Effect]) -> LoadRequest {
    let requests: Vec<&LoadRequest> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LoadChildren(request) => Some(request),
            _ => None,
        })
        .collect();
    match requests.as_slice() {
        [request] => (*request).clone(),
        other => panic!("expected one tree listing, got {other:?} in {effects:?}"),
    }
}

fn tree_labels(state: &AppState) -> Vec<String> {
    state
        .tree()
        .rows()
        .into_iter()
        .map(|row| row.label)
        .collect()
}

#[test]
fn the_tree_follows_the_active_pane() {
    let mut state = ready(vec![], vec![]);
    // The places come; the tree lists the place that holds the left pane's folder.
    let mut effects = state.set_places(&[drive("임시", &temp())]);
    effects.extend(state.follow_active());
    let request = the_tree_load(&effects);
    assert_eq!(request.path, temp());
    assert!(state.generations().contains(&request.generation));
    assert_eq!(state.tree().selected_row(), None);

    // Its listing selects the pane's folder and scrolls the tree to it.
    let effects = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("a"), dir("b"), dir("c"), file("x.txt")],
    });
    assert_eq!(effects, [Effect::ScrollTree(1)]);
    assert_eq!(tree_labels(&state), ["임시", "a", "b", "c"]);
    assert!(!state.generations().contains(&request.generation));
    // Nothing to do while the pane stays where it is.
    assert_eq!(state.follow_active(), []);

    // The right pane gets the keyboard: the tree selects its folder.
    press(&mut state, &special(Key::Tab), NONE);
    assert_eq!(state.follow_active(), [Effect::ScrollTree(2)]);
    // A click on a row shows its folder in the active pane; the row is selected already.
    let effects = state.open_tree_row(3);
    assert_eq!(the_load(&effects).path, folder("c"));
    assert_eq!(state.active(), RIGHT);
    assert_eq!(state.tree().selected_row(), Some(3));
    assert_eq!(state.follow_active(), []);
    // A folder under no place selects nothing and lists nothing.
    let elsewhere =
        VPath::new(rf_core::Scheme::Remote, "host", &["share"]).expect("a remote address");
    state.go_to(elsewhere);
    assert_eq!(state.follow_active(), []);
    assert_eq!(state.tree().selected(), None);
    assert_eq!(state.open_tree_row(99), []);
}

#[test]
fn tree_rows_open_once_and_follow_the_hidden_setting() {
    let mut state = ready(vec![], vec![]);
    assert_eq!(state.set_places(&[drive("임시", &temp())]), []);
    let request = the_tree_load(&state.toggle_tree_row(0));
    assert_eq!(request.path, temp());
    // Closing and opening again asks for nothing more.
    assert_eq!(state.toggle_tree_row(0), []);
    assert_eq!(state.toggle_tree_row(0), []);
    assert_eq!(state.toggle_tree_row(99), []);
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![
            {
                let mut dot = dir(".h");
                dot.attributes |= Attributes::HIDDEN;
                dot
            },
            dir("a"),
        ],
    });
    assert_eq!(tree_labels(&state), ["임시", "a"]);
    // Ctrl+H shows the hidden folders in the tree too, dimmed as in the list.
    assert_eq!(press(&mut state, "h", CTRL), Some(vec![]));
    assert_eq!(tree_labels(&state), ["임시", ".h", "a"]);
    let hidden: Vec<bool> = state.tree().rows().iter().map(|row| row.hidden).collect();
    assert_eq!(hidden, [false, true, false]);

    // A listing that failed leaves the row empty, without a notice.
    let a = the_tree_load(&state.toggle_tree_row(2));
    let effects = state.loaded(LoadEvent::Failed {
        generation: a.generation,
        kind: ErrorKind::PermissionDenied,
    });
    assert_eq!(effects, []);
    assert!(!state.tree().rows()[2].has_children);
    assert!(!state.generations().contains(&a.generation));
}

#[test]
fn the_tree_follows_folder_changes() {
    let mut state = ready(vec![], vec![]);
    let mut effects = state.set_places(&[drive("임시", &temp())]);
    effects.extend(state.follow_active());
    let request = the_tree_load(&effects);
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("a"), dir("b")],
    });
    assert_eq!(tree_labels(&state), ["임시", "a", "b"]);

    // The left pane shows the temporary folder: its listing, then its watch, keep the
    // tree's node of that folder up to date.
    let load = the_load(&state.go_to(temp()));
    state.loaded(LoadEvent::Listed {
        generation: load.generation,
        entries: vec![file("x.txt"), dir("new"), dir("b"), dir("a")],
    });
    assert_eq!(tree_labels(&state), ["임시", "a", "b", "new"]);
    state.loaded(LoadEvent::Changes {
        generation: load.generation,
        changes: vec![rf_vfs::Change::Removed("b".into())],
    });
    assert_eq!(tree_labels(&state), ["임시", "a", "new"]);
    // A job's folder that a pane shows follows through that pane's watch.
    assert_eq!(state.folders_changed(&[temp()]), []);

    // One that no pane shows is listed again, once the tree has listed it.
    state.go_to(folder("a"));
    let effects = state.folders_changed(&[temp(), folder("never-listed")]);
    let request = the_tree_load(&effects);
    assert_eq!(request.path, temp());
    assert!(state.generations().contains(&request.generation));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("a"), dir("made"), dir("new")],
    });
    assert_eq!(tree_labels(&state), ["임시", "a", "made", "new"]);
}

/// A window state whose left pane at `a` lists nothing and whose right pane at the temporary
/// folder lists `right`, with the temporary folder as the tree's place; and the generation of
/// the right pane's listing, which its watch's changes carry.
fn right_at_temp(right: Vec<Entry>) -> (AppState, u64) {
    let (mut state, effects) = AppState::new(folder("a"), temp(), Some(folder("home")));
    let requests: Vec<LoadRequest> = loads(&effects).into_iter().cloned().collect();
    for (request, entries) in requests.iter().zip([vec![], right]) {
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries,
        });
    }
    assert_eq!(state.set_places(&[drive("임시", &temp())]), []);
    (state, requests[1].generation)
}

#[test]
fn a_change_the_tree_misses_while_it_lists_a_folder_comes_from_the_pane() {
    let (mut state, right) = right_at_temp(vec![dir("x")]);
    let request = the_tree_load(&state.toggle_tree_row(0));
    // The right pane's watch tells of a new folder while the tree's listing is on its way, and
    // that listing was read before the folder was made.
    state.loaded(LoadEvent::Changes {
        generation: right,
        changes: vec![rf_vfs::Change::Upsert(dir("new"))],
    });
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("x")],
    });
    assert_eq!(tree_labels(&state), ["임시", "new", "x"]);
}

#[test]
fn the_tree_goes_on_when_a_pane_lists_the_folder_it_lacked() {
    // The left pane shows `a`, made after the right pane listed the temporary folder.
    let (mut state, right) = right_at_temp(vec![dir("x")]);
    let request = the_tree_load(&state.follow_active());
    let effects = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("x")],
    });
    // The way ends there: that listing is new, so it is not made again.
    assert_eq!(effects, []);
    assert_eq!(state.tree().selected(), None);
    assert_eq!(state.tree().generations().count(), 0);

    // The right pane's watch tells of `a`: the tree finds it, and scrolls to it.
    let effects = state.loaded(LoadEvent::Changes {
        generation: right,
        changes: vec![rf_vfs::Change::Upsert(dir("a"))],
    });
    assert_eq!(effects, [Effect::ScrollTree(1)]);
    assert_eq!(tree_labels(&state), ["임시", "a", "x"]);
    assert_eq!(state.tree().selected_row(), Some(1));
}

#[test]
fn start_folders_fall_back_to_home() {
    let (home, docs, root) = (folder("home"), folder("docs"), folder("root"));
    let cwd = std::env::temp_dir();
    assert_eq!(
        start_folders(None, Some(&cwd), Some(&home), Some(&docs), Some(&root)),
        Some((home.clone(), docs.clone()))
    );
    assert_eq!(
        start_folders(
            Some(Path::new("rel")),
            Some(&cwd),
            Some(&home),
            None,
            Some(&root)
        ),
        Some((folder("rel"), home.clone()))
    );
    let absolute = std::env::temp_dir().join("abs");
    assert_eq!(
        start_folders(Some(&absolute), None, Some(&home), None, None),
        Some((folder("abs"), home))
    );
    assert_eq!(
        start_folders(None, None, None, None, Some(&root)),
        Some((root.clone(), root))
    );
    assert_eq!(start_folders(None, None, None, None, None), None);
}

#[test]
fn the_filter_box_filters_the_active_pane() {
    let mut state = ready(
        vec![file("보고서.txt"), file("메모.txt"), file("보고-2.md")],
        vec![file("보고서.txt")],
    );
    state.click_row(LEFT, 1, false, false);
    let effects = state.filter_edited("보고");
    assert_eq!(rows(&state, LEFT), ["보고서.txt", "보고-2.md"]);
    // The other pane is left alone.
    assert_eq!(rows(&state, RIGHT), ["보고서.txt"]);
    assert_eq!(state.filter_text(), "보고");
    // The cursor stays where its row was, on the nearest row left, and shows.
    assert_eq!(cursor(&state, LEFT), Some(1));
    assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 1 }]);
    // The box shows the active pane's own filter.
    state.activate(RIGHT);
    assert_eq!(state.filter_text(), "");
    state.activate(LEFT);
    assert_eq!(state.filter_text(), "보고");
    // Esc in the box clears it.
    state.filter_edited("");
    assert_eq!(rows(&state, LEFT).len(), 3);
    assert_eq!(state.filter_text(), "");
}

#[test]
fn up_puts_the_cursor_on_the_folder_it_came_from_and_a_new_folder_starts_at_its_top() {
    let mut state = ready(vec![dir("x"), dir("y")], vec![]);
    // Into `y`: its rows come with the cursor on the first one, nothing selected.
    let effects = state.open_row(LEFT, 1);
    let request = the_load(&effects);
    assert_eq!(request.path, folder("a").join("y").expect("a name"));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("p"), file("q")],
    });
    assert_eq!(cursor(&state, LEFT), Some(0));
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    // An empty folder has no row for it.
    let effects = state.open_row(LEFT, 0);
    let empty = the_load(&effects);
    state.loaded(LoadEvent::Listed {
        generation: empty.generation,
        entries: Vec::new(),
    });
    assert_eq!(cursor(&state, LEFT), None);

    // Up twice: back in `a`, the cursor is on `y`, which shows.
    let request = the_load(&press(&mut state, &special(Key::UpArrow), ALT).expect("used"));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("p"), file("q")],
    });
    assert_eq!(selected(&state, LEFT), [0], "back in `y`, on `p`");
    let request = the_load(&press(&mut state, &special(Key::UpArrow), ALT).expect("used"));
    assert_eq!(request.path, folder("a"));
    let effects = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![dir("x"), dir("y")],
    });
    assert_eq!(cursor(&state, LEFT), Some(1));
    assert_eq!(selected(&state, LEFT), [1]);
    assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 1 }]);
}
