//! The selection tools (NAV-15), listing a folder again (NAV-13) and the extensions shown or not
//! (NAV-11), from the keys and the pattern window as the controller drives them.

use super::*;
use crate::dialogs::{Counting, PatternDialog};
use crate::pane::INLINE_SORT_ROWS;
use crate::pattern::{PatternError, PatternSyntax};

fn hidden(mut entry: Entry) -> Entry {
    entry.attributes |= Attributes::HIDDEN;
    entry
}

/// The names of the selected rows of pane `side`.
fn selected_names(state: &AppState, side: usize) -> Vec<&str> {
    let tab = state.panes()[side].active_tab();
    tab.selection()
        .indices(tab.visible())
        .into_iter()
        .map(|i| tab.visible()[i].name.to_str().expect("test names are text"))
        .collect()
}

/// The left pane's rows: a folder, files of three extensions, a hidden one.
fn photos() -> Vec<Entry> {
    vec![
        dir("사진"),
        file("a.jpg"),
        file("b.PNG"),
        file("c.JPG"),
        hidden(file(".h.jpg")),
        file("d.txt"),
        file("README"),
    ]
}

fn pattern_dialog(state: &AppState) -> &PatternDialog {
    match state.modal() {
        Some(Modal::Pattern(dialog)) => dialog,
        other => panic!("the pattern window, not {other:?}"),
    }
}

/// The one request for a worker to match names among `effects`.
fn the_match(effects: &[Effect]) -> MatchRequest {
    match effects {
        [Effect::MatchNames(request)] => request.clone(),
        other => panic!("one match on a worker, not {other:?}"),
    }
}

fn names(request: &MatchRequest) -> Vec<&str> {
    request
        .rows
        .iter()
        .map(|name| name.to_str().expect("test names are text"))
        .collect()
}

const ALL_MODES: [ViewMode; 5] = [
    ViewMode::Details,
    ViewMode::List,
    ViewMode::SmallIcons,
    ViewMode::MediumIcons,
    ViewMode::LargeIcons,
];

#[test]
fn all_none_and_the_inverse_are_of_the_rows_on_screen_in_every_view() {
    for mode in ALL_MODES {
        let mut state = ready(photos(), vec![]);
        state.set_view_mode(LEFT, mode);
        // The hidden file is not on screen: nothing selects it.
        press(&mut state, "a", CTRL);
        assert_eq!(
            selected_names(&state, LEFT),
            ["사진", "a.jpg", "b.PNG", "c.JPG", "d.txt", "README"],
            "{mode:?}"
        );
        press(&mut state, &special(Key::Escape), NONE);
        assert_eq!(selected_names(&state, LEFT), Vec::<&str>::new());
        state.click_row(LEFT, 1, false, false);
        press(&mut state, "I", CTRL_SHIFT);
        assert_eq!(
            selected_names(&state, LEFT),
            ["사진", "b.PNG", "c.JPG", "d.txt", "README"],
            "{mode:?}"
        );
        // The filter box leaves two rows: the inverse is among them alone.
        state.filter_edited("jpg");
        assert_eq!(rows(&state, LEFT), ["a.jpg", "c.JPG"]);
        assert_eq!(selected_names(&state, LEFT), ["c.JPG"]);
        press(&mut state, "I", CTRL_SHIFT);
        assert_eq!(selected_names(&state, LEFT), ["a.jpg"]);
        press(&mut state, "a", CTRL);
        assert_eq!(selected_names(&state, LEFT), ["a.jpg", "c.JPG"]);
        // Once the filter goes, the rows it hid were not selected.
        state.filter_edited("");
        assert_eq!(selected_names(&state, LEFT), ["a.jpg", "c.JPG"], "{mode:?}");
    }
}

#[test]
fn the_menu_s_items_do_what_their_keys_do() {
    let mut state = ready(photos(), vec![]);
    let menu = |state: &mut AppState, command: Command| {
        state.key(command, Mods::default(), Paging::rows(1), Instant::now())
    };
    assert_eq!(menu(&mut state, Command::SelectAll), Some(vec![]));
    assert_eq!(selected(&state, LEFT).len(), 6);
    assert_eq!(menu(&mut state, Command::InvertSelection), Some(vec![]));
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    assert_eq!(menu(&mut state, Command::SelectPattern), Some(vec![]));
    assert!(matches!(state.modal(), Some(Modal::Pattern(_))));
    // While a window shows, the keys do nothing.
    assert_eq!(press(&mut state, "a", CTRL), Some(vec![]));
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
}

#[test]
fn an_inverted_selection_leaves_no_file_command_on_the_cursor_s_row() {
    let mut state = ready(vec![file("a"), file("b"), file("c")], vec![]);
    state.click_row(LEFT, 1, false, false);
    press(&mut state, "I", CTRL_SHIFT);
    assert_eq!(selected_names(&state, LEFT), ["a", "c"]);
    assert_eq!(cursor(&state, LEFT), Some(1));
    // F2 renames the first selected row, not the cursor's, which shows unselected.
    press(&mut state, &special(Key::F2), NONE);
    let Some(Modal::Name(dialog)) = state.modal() else {
        panic!("the name window");
    };
    assert_eq!(dialog.initial, "a");
    state.dialog_cancelled();
    // Delete takes the selection.
    let effects = press(&mut state, &special(Key::Delete), NONE).expect("used");
    let [Effect::CheckTrash { paths, .. }] = effects.as_slice() else {
        panic!("a trash: {effects:?}");
    };
    let named: Vec<_> = paths.iter().filter_map(VPath::file_name).collect();
    assert_eq!(named, ["a", "c"]);
}

#[test]
fn the_extension_of_the_cursor_s_row_selects_its_like_ignoring_case() {
    for mode in ALL_MODES {
        let mut state = ready(photos(), vec![]);
        state.set_view_mode(LEFT, mode);
        state.click_row(LEFT, 2, false, false);
        state.click_row(LEFT, 5, true, false);
        // On c.JPG: the other jpg too, not the hidden one; the rest goes.
        state.click_row(LEFT, 3, true, false);
        assert_eq!(press(&mut state, "S", CTRL_SHIFT), Some(vec![]));
        assert_eq!(selected_names(&state, LEFT), ["a.jpg", "c.JPG"], "{mode:?}");
        assert_eq!(
            state.panes()[LEFT].active_tab().selection().user_cursor(),
            Some(3)
        );
    }
    // On a row without an extension: nothing changes, and a notice says why.
    let mut state = ready(photos(), vec![]);
    state.click_row(LEFT, 1, false, false);
    state.click_row(LEFT, 5, true, false);
    let effects = press(&mut state, "S", CTRL_SHIFT).expect("used");
    assert_eq!(effects, [Effect::Notice(words::no_extension("README"))]);
    assert_eq!(selected_names(&state, LEFT), ["a.jpg", "README"]);
    // The folder the list put the cursor on has none either.
    let mut fresh = ready(photos(), vec![]);
    let effects = press(&mut fresh, "S", CTRL_SHIFT).expect("used");
    assert_eq!(effects, [Effect::Notice(words::no_extension("사진"))]);
    // An empty folder has no cursor: nothing.
    let mut empty = ready(vec![], vec![]);
    assert_eq!(press(&mut empty, "S", CTRL_SHIFT), Some(vec![]));
}

#[test]
fn extensions_beyond_ascii_are_alike_ignoring_case() {
    let mut state = ready(
        vec![
            file("É.JPG"),
            file("é.jpg"),
            file("PHOTO.ÉPS"),
            file("x.éps"),
            file("y.eps"),
        ],
        vec![],
    );
    let names = rows(&state, LEFT);
    let at = |name: &str| names.iter().position(|row| *row == name).expect("a row");
    let (jpg, eps) = (at("É.JPG"), at("PHOTO.ÉPS"));
    state.click_row(LEFT, jpg, false, false);
    press(&mut state, "S", CTRL_SHIFT);
    assert_eq!(selected_names(&state, LEFT).len(), 2);
    assert!(selected_names(&state, LEFT).contains(&"é.jpg"));
    state.click_row(LEFT, eps, false, false);
    press(&mut state, "S", CTRL_SHIFT);
    let mut chosen = selected_names(&state, LEFT);
    chosen.sort_unstable();
    assert_eq!(chosen, ["PHOTO.ÉPS", "x.éps"]);
}

#[test]
fn the_pattern_window_counts_on_a_worker_and_drops_late_answers() {
    let mut state = ready(photos(), vec![file("x.jpg")]);
    // Nothing typed yet: nothing to count, and 선택 is off.
    assert_eq!(press(&mut state, "s", CTRL), Some(vec![]));
    let dialog = pattern_dialog(&state);
    assert_eq!((dialog.side, dialog.choice.text.as_str()), (LEFT, ""));
    assert!(dialog.empty && !dialog.can_confirm());

    // The rows on screen go to the worker, the hidden one not among them.
    let first = the_match(&state.pattern_edited("*.jpg"));
    assert_eq!(
        names(&first),
        ["사진", "a.jpg", "b.PNG", "c.JPG", "d.txt", "README"]
    );
    assert_eq!(pattern_dialog(&state).counting, Counting::Running);
    let second = the_match(&state.pattern_edited("*.jpg;*.png"));
    assert_ne!(first.generation, second.generation);
    // The rows are taken once: the second request has the same ones.
    assert!(Arc::ptr_eq(&first.rows, &second.rows));
    // An answer for a pattern typed over is dropped.
    let late = first.run(&|| true).expect("not stopped");
    assert_eq!(late.hits.len(), 2);
    assert_eq!(state.pattern_matched(late), []);
    assert_eq!(pattern_dialog(&state).count, None);
    let answer = second.run(&|| true).expect("not stopped");
    assert_eq!(state.pattern_matched(answer.clone()), []);
    let dialog = pattern_dialog(&state);
    assert_eq!((dialog.count, dialog.counting), (Some(3), Counting::Idle));
    // The same answer twice changes nothing.
    assert_eq!(state.pattern_matched(answer), []);

    // Case counts when 대소문자 무시 is off: the worker counts again.
    let exact = the_match(&state.pattern_case(false));
    assert_eq!(
        pattern_dialog(&state).count,
        Some(3),
        "the last count meanwhile"
    );
    state.pattern_matched(exact.run(&|| true).expect("not stopped"));
    assert_eq!(pattern_dialog(&state).count, Some(1));
    // The mode is no change of the pattern.
    state.pattern_mode(true);
    assert_eq!(pattern_dialog(&state).counting, Counting::Idle);
}

#[test]
fn a_text_that_is_no_pattern_says_why_and_selects_nothing() {
    let mut state = ready(photos(), vec![]);
    press(&mut state, "s", CTRL);
    let _ = state.pattern_edited("(jpg");
    // As wildcards it is a name; as a regex it is not.
    assert_eq!(pattern_dialog(&state).problem, None);
    assert_eq!(state.pattern_syntax(true), []);
    let dialog = pattern_dialog(&state);
    assert_eq!(dialog.choice.syntax, PatternSyntax::Regex);
    assert_eq!(dialog.problem, Some(PatternError::Invalid));
    assert!(!dialog.can_confirm());
    assert_eq!(dialog.counting, Counting::Idle);
    assert_eq!(state.pattern_confirmed("(jpg"), []);
    assert!(matches!(state.modal(), Some(Modal::Pattern(_))));
    // Mended, it counts again.
    let request = the_match(&state.pattern_edited(r"\.jpg$"));
    assert_eq!(pattern_dialog(&state).problem, None);
    state.pattern_matched(request.run(&|| true).expect("not stopped"));
    assert_eq!(pattern_dialog(&state).count, Some(2));
    // Too long: said, and nothing asked.
    let long = "a".repeat(crate::pattern::MAX_PATTERN_CHARS + 1);
    assert_eq!(state.pattern_edited(&long), []);
    assert_eq!(pattern_dialog(&state).problem, Some(PatternError::TooLong));
    // Esc closes it, selecting nothing.
    state.dialog_cancelled();
    assert_eq!(state.modal(), None);
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
}

#[test]
fn choose_selects_what_the_worker_matched_once_it_is_done() {
    let mut state = ready(photos(), vec![]);
    state.click_row(LEFT, 4, false, false);
    press(&mut state, "s", CTRL);
    let request = the_match(&state.pattern_edited("*.jpg"));
    // 선택 before the worker is done: the window waits for it, and 선택 is off meanwhile.
    assert_eq!(state.pattern_confirmed("*.jpg"), []);
    assert!(pattern_dialog(&state).confirming);
    assert!(!pattern_dialog(&state).can_confirm());
    assert_eq!(selected_names(&state, LEFT), ["d.txt"]);
    let effects = state.pattern_matched(request.run(&|| true).expect("not stopped"));
    assert_eq!(effects, []);
    assert_eq!(state.modal(), None);
    // In place of the selection, by default; the cursor stays, on a row nobody chose now.
    assert_eq!(selected_names(&state, LEFT), ["a.jpg", "c.JPG"]);
    let tab = state.panes()[LEFT].active_tab();
    assert_eq!(
        (tab.selection().cursor(), tab.selection().user_cursor()),
        (Some(4), None)
    );
}

#[test]
fn the_next_window_starts_with_the_last_choices_and_can_add_to_the_selection() {
    let mut state = ready(photos(), vec![]);
    state.click_row(LEFT, 4, false, false);
    press(&mut state, "s", CTRL);
    let _ = state.pattern_edited("*.png");
    state.pattern_mode(true);
    // Text typed and confirmed in one go (Enter right after a key): the worker counts it first.
    let effects = state.pattern_confirmed("*.png;readme");
    let request = the_match(&effects);
    assert!(pattern_dialog(&state).confirming);
    state.pattern_matched(request.run(&|| true).expect("not stopped"));
    assert_eq!(state.modal(), None);
    assert_eq!(selected_names(&state, LEFT), ["b.PNG", "d.txt", "README"]);

    // The next window starts with what this one chose, and counts it at once.
    let effects = press(&mut state, "s", CTRL).expect("used");
    let dialog = pattern_dialog(&state);
    assert_eq!(dialog.initial, "*.png;readme");
    assert!(dialog.choice.add);
    let request = the_match(&effects);
    // Cancelled, it leaves the last choices as they were.
    let _ = state.pattern_edited("*.txt");
    state.dialog_cancelled();
    assert_eq!(
        state.pattern_matched(request.run(&|| true).expect("run")),
        []
    );
    press(&mut state, "s", CTRL);
    assert_eq!(pattern_dialog(&state).initial, "*.png;readme");
}

#[test]
fn the_pattern_window_matches_again_when_its_rows_change() {
    let mut state = ready(vec![file("a.txt"), file("b.jpg")], vec![]);
    press(&mut state, "s", CTRL);
    let before = the_match(&state.pattern_edited("*.txt"));
    let generation = state.panes()[LEFT].active_tab().generation();
    // Another app makes a file: the worker matches the rows as they are now.
    let effects = state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(file("n.txt"))],
    });
    let after = the_match(&effects);
    assert_eq!(names(&after), ["a.txt", "b.jpg", "n.txt"]);
    assert_eq!(
        state.pattern_matched(before.run(&|| true).expect("run")),
        []
    );
    // 선택 waits for the new answer, then selects the new file too.
    assert_eq!(state.pattern_confirmed("*.txt"), []);
    state.pattern_matched(after.run(&|| true).expect("run"));
    assert_eq!(selected_names(&state, LEFT), ["a.txt", "n.txt"]);
    // A change in the other pane asks for nothing.
    let mut other = ready(vec![file("a.txt")], vec![file("z.txt")]);
    press(&mut other, "s", CTRL);
    let _ = other.pattern_edited("*.txt");
    let right = other.panes()[RIGHT].active_tab().generation();
    let effects = other.loaded(LoadEvent::Changes {
        generation: right,
        changes: vec![rf_vfs::Change::Upsert(file("y.txt"))],
    });
    assert_eq!(effects, []);
}

#[test]
fn refresh_lists_again_and_keeps_the_selection_the_cursor_and_the_place() {
    let mut state = ready(vec![file("a"), file("b"), file("c"), file("d")], vec![]);
    state.click_row(LEFT, 1, false, false);
    state.click_row(LEFT, 3, true, false);
    let before = state.panes()[LEFT].active_tab().generation();
    let effects = press(&mut state, &special(Key::F5), NONE).expect("used");
    let request = the_load(&effects);
    assert_eq!(effects.len(), 1, "no scroll: {effects:?}");
    assert_eq!(request.path, folder("a"));
    assert_ne!(request.generation, before);
    // The rows stay while it lists.
    assert_eq!(rows(&state, LEFT), ["a", "b", "c", "d"]);
    // A's gone and e is new: b and d stay selected, the cursor stays on d, nothing scrolls.
    let effects = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: vec![file("b"), file("c"), file("d"), file("e")],
    });
    assert_eq!(effects, []);
    assert_eq!(selected_names(&state, LEFT), ["b", "d"]);
    let tab = state.panes()[LEFT].active_tab();
    assert_eq!(tab.selection().user_cursor(), Some(2));
    // A listing of the request before it comes late: dropped.
    assert_eq!(
        state.loaded(LoadEvent::Listed {
            generation: before,
            entries: vec![file("old")],
        }),
        []
    );
    assert_eq!(rows(&state, LEFT), ["b", "c", "d", "e"]);
    // macOS's key, and 다시 시도 of a listing that failed, do the same.
    let effects = state
        .key(
            keys::command_for("r", CTRL, Os::MacOs).expect("⌘R"),
            CTRL,
            Paging::rows(1),
            Instant::now(),
        )
        .expect("used");
    assert_eq!(the_load(&effects).path, folder("a"));
    let effects = state.retry(LEFT);
    assert_eq!(the_load(&effects).path, folder("a"));
}

#[test]
fn refresh_drops_a_sort_under_way_and_lists_in_the_view_s_order() {
    let many: Vec<Entry> = (0..=INLINE_SORT_ROWS)
        .map(|i| file(&format!("f{i:04}")))
        .collect();
    let mut state = ready(many, vec![]);
    let mut settled = Vec::new();
    state.settle_views(&mut settled);
    assert_eq!(settled, []);
    // Name again: the other way round, which a worker sorts.
    let mut effects = state.sort_by(LEFT, SortKey::Name, false);
    state.settle_views(&mut effects);
    let [Effect::Sort(sort)] = effects.as_slice() else {
        panic!("a sort on a worker: {effects:?}");
    };
    let sorted = sort.clone().run();
    // F5 while it sorts: the listing comes in the new order, and the old rows' sort is dropped.
    let mut effects = press(&mut state, &special(Key::F5), NONE).expect("used");
    state.settle_views(&mut effects);
    let request = the_load(&effects);
    assert_eq!(request.sort, state.panes()[LEFT].active_tab().sort());
    assert_eq!(state.sorted(sorted), []);
    assert_eq!(rows(&state, LEFT)[0], "f0000");
    // The loader's worker sorts the listing in that order: no sort follows it.
    let entries: Vec<Entry> = std::iter::once(file("new"))
        .chain(
            (0..=INLINE_SORT_ROWS)
                .rev()
                .map(|i| file(&format!("f{i:04}"))),
        )
        .collect();
    let mut effects = state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries,
    });
    state.settle_views(&mut effects);
    assert_eq!(effects, []);
    assert_eq!(rows(&state, LEFT)[..2], ["new", "f2000"]);
}

#[test]
fn hidden_extensions_are_kept_and_rename_edits_the_whole_name() {
    let mut state = ready(vec![file("설계안.pdf")], vec![]);
    assert!(state.show_extensions());
    let toggle = |state: &mut AppState| {
        state.key(
            Command::ToggleExtensions,
            Mods::default(),
            Paging::rows(1),
            Instant::now(),
        )
    };
    assert_eq!(toggle(&mut state), Some(vec![]));
    assert!(!state.show_extensions());
    assert!(state.session().hide_extensions);
    // The rows are what they were: only the screen leaves the extension out.
    assert_eq!(rows(&state, LEFT), ["설계안.pdf"]);
    state.click_row(LEFT, 0, false, false);
    press(&mut state, &special(Key::F2), NONE);
    let Some(Modal::Name(dialog)) = state.modal() else {
        panic!("the name window");
    };
    assert_eq!(dialog.initial, "설계안.pdf");
    assert_eq!(&dialog.initial[..dialog.select_end], "설계안");
    state.dialog_cancelled();
    // A window opened from that session hides them too; toggled again, it shows them.
    let (restored, _) = AppState::restore(folder("a"), folder("b"), state.session(), &[], None);
    assert!(!restored.show_extensions());
    toggle(&mut state);
    assert!(state.show_extensions() && !state.session().hide_extensions);
}

#[test]
fn a_count_the_worker_could_not_make_is_matched_here_when_choose_is_pressed() {
    let mut state = ready(photos(), vec![]);
    press(&mut state, "s", CTRL);
    let request = the_match(&state.pattern_edited("*.jpg"));
    // The worker panicked, or no thread could be started for it: the window says so, and
    // waits no more.
    assert_eq!(state.pattern_failed(request.generation), []);
    let dialog = pattern_dialog(&state);
    assert_eq!(dialog.counting, Counting::Failed);
    assert!(dialog.can_confirm());
    assert_eq!(
        crate::ui_bridge::pattern_view(dialog).count,
        words::pattern_uncounted()
    );
    // 선택 matches them here, and selects them.
    assert_eq!(state.pattern_confirmed("*.jpg"), []);
    assert_eq!(state.modal(), None);
    assert_eq!(selected_names(&state, LEFT), ["a.jpg", "c.JPG"]);

    // Pressed while the worker counted, the failure selects at once; a failure of a pattern
    // typed over changes nothing.
    press(&mut state, "s", CTRL);
    let old = the_match(&state.pattern_edited("*.png"));
    let new = the_match(&state.pattern_edited("*.txt"));
    assert_eq!(state.pattern_failed(old.generation), []);
    assert_eq!(pattern_dialog(&state).counting, Counting::Running);
    assert_eq!(state.pattern_confirmed("*.txt"), []);
    assert!(pattern_dialog(&state).confirming);
    assert_eq!(state.pattern_failed(new.generation), []);
    assert_eq!(state.modal(), None);
    assert_eq!(selected_names(&state, LEFT), ["d.txt"]);
}

#[test]
fn a_sort_the_worker_could_not_make_leaves_the_rows_in_their_order() {
    let many: Vec<Entry> = (0..=INLINE_SORT_ROWS)
        .map(|i| file(&format!("f{i:04}")))
        .collect();
    let mut state = ready(many, vec![]);
    let mut settled = Vec::new();
    state.settle_views(&mut settled);
    let mut effects = state.sort_by(LEFT, SortKey::Name, false);
    state.settle_views(&mut effects);
    let [Effect::Sort(sort)] = effects.as_slice() else {
        panic!("a sort on a worker: {effects:?}");
    };
    let sort = sort.clone();
    // A failure of another sort changes nothing.
    assert_eq!(state.sort_failed(sort.generation, sort.serial + 1), []);
    // The tab stops waiting: its rows keep their order, which the header shows again, and no
    // worker is asked for the same sort.
    assert_eq!(
        state.sort_failed(sort.generation, sort.serial),
        [Effect::Notice(words::sort_failed().to_owned())]
    );
    let tab = state.panes()[LEFT].active_tab();
    assert_eq!(tab.sort().primary.order, SortOrder::Ascending);
    assert_eq!(rows(&state, LEFT)[0], "f0000");
    let mut effects = Vec::new();
    state.settle_views(&mut effects);
    assert_eq!(effects, []);
    // A late answer of that sort is dropped.
    assert_eq!(state.sorted(sort.run()), []);
    assert_eq!(rows(&state, LEFT)[0], "f0000");
}

#[test]
fn a_choice_that_waits_for_the_count_waits_through_a_change_of_the_rows() {
    let mut state = ready(vec![file("a.txt"), file("b.jpg")], vec![]);
    press(&mut state, "s", CTRL);
    let before = the_match(&state.pattern_edited("*.txt"));
    // 선택 while the worker counts; then another app makes a file before it answers.
    assert_eq!(state.pattern_confirmed("*.txt"), []);
    let generation = state.panes()[LEFT].active_tab().generation();
    let after = the_match(&state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(file("n.txt"))],
    }));
    assert!(pattern_dialog(&state).confirming);
    // The answer for the rows before is dropped; the one for the rows now selects them.
    assert_eq!(
        state.pattern_matched(before.run(&|| true).expect("run")),
        []
    );
    assert!(matches!(state.modal(), Some(Modal::Pattern(_))));
    assert_eq!(state.pattern_matched(after.run(&|| true).expect("run")), []);
    assert_eq!(state.modal(), None);
    assert_eq!(selected_names(&state, LEFT), ["a.txt", "n.txt"]);
    // A change of the pattern after 선택 drops it: the press was for the pattern before.
    press(&mut state, "s", CTRL);
    let _ = state.pattern_edited("*.jpg");
    assert_eq!(state.pattern_confirmed("*.jpg"), []);
    let _ = state.pattern_edited("*.png");
    assert!(!pattern_dialog(&state).confirming);
}

/// A window state like [`ready`] with a job that runs, so that closing the window asks first.
fn ready_with_a_job(left: Vec<Entry>) -> AppState {
    let mut state = ready(left, vec![]);
    state.track(crate::jobs::Started {
        id: JobId(1),
        spec: None,
        work: words::JobWork::Trash,
        origin: None,
        via_history: false,
    });
    state
}

#[test]
fn a_pattern_window_behind_the_quit_question_keeps_up_and_comes_back_counted() {
    let mut state = ready_with_a_job(vec![file("a.txt"), file("b.jpg")]);
    press(&mut state, "s", CTRL);
    let first = the_match(&state.pattern_edited("*.txt"));
    // The window's close button: the question whether to quit shows over the pattern window.
    assert_eq!(state.request_close(), []);
    assert!(matches!(state.modal(), Some(Modal::Confirm(_))));
    // Meanwhile the worker answers, and the rows change: the waiting window takes both.
    assert_eq!(state.pattern_matched(first.run(&|| true).expect("run")), []);
    let generation = state.panes()[LEFT].active_tab().generation();
    let again = the_match(&state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(file("n.txt"))],
    }));
    assert_eq!(state.pattern_matched(again.run(&|| true).expect("run")), []);
    // Staying: the window shows again, counted, and 선택 selects at once.
    assert_eq!(state.dialog_cancelled(), []);
    let dialog = pattern_dialog(&state);
    assert_eq!((dialog.count, dialog.counting), (Some(2), Counting::Idle));
    assert_eq!(state.pattern_confirmed("*.txt"), []);
    assert_eq!(state.modal(), None);
    assert_eq!(selected_names(&state, LEFT), ["a.txt", "n.txt"]);
}

#[test]
fn a_choice_made_before_the_quit_question_selects_when_the_count_comes() {
    let mut state = ready_with_a_job(vec![file("a.txt"), file("b.jpg")]);
    press(&mut state, "s", CTRL);
    let request = the_match(&state.pattern_edited("*.txt"));
    assert_eq!(state.pattern_confirmed("*.txt"), []);
    state.request_close();
    // The answer comes while the question shows: the rows are selected, the waiting window
    // goes, and the question stays.
    assert_eq!(
        state.pattern_matched(request.run(&|| true).expect("run")),
        []
    );
    assert!(matches!(state.modal(), Some(Modal::Confirm(_))));
    assert_eq!(selected_names(&state, LEFT), ["a.txt"]);
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);
}
