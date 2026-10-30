//! The preview panel (VIEW-02) and quick look (VIEW-01): what they follow, the requests for a
//! worker and their generations, the keys of quick look, and Space while a name is typed.

use std::time::Duration;

use super::*;
use crate::keys::QuickLookKey;
use crate::preview::{PreviewContent, Previewed};
use rf_vfs::Change;

const ALL_MODES: [ViewMode; 5] = [
    ViewMode::Details,
    ViewMode::List,
    ViewMode::SmallIcons,
    ViewMode::MediumIcons,
    ViewMode::LargeIcons,
];

/// The one request for a preview among `effects`.
fn the_preview(effects: &[Effect]) -> PreviewRequest {
    let requests: Vec<&PreviewRequest> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Preview(request) => Some(request),
            _ => None,
        })
        .collect();
    match requests.as_slice() {
        [request] => (*request).clone(),
        other => panic!("one preview, not {other:?} in {effects:?}"),
    }
}

/// The name of the item the preview shows.
fn previewed_name(state: &AppState) -> Option<&str> {
    state
        .preview()
        .map(|shown| shown.entry.name.to_str().expect("test names are text"))
}

/// The rows of the left pane: folders and files mixed, as a sort without folders first gives.
fn mixed() -> Vec<Entry> {
    vec![
        dir("d1"),
        file("a.txt"),
        dir("d2"),
        file("b.png"),
        file("c.md"),
    ]
}

/// The preview panel shown, and what it asks for first.
fn with_panel(left: Vec<Entry>, right: Vec<Entry>) -> (AppState, Vec<Effect>) {
    let mut state = ready(left, right);
    assert_eq!(state.follow_preview(), [], "nothing while nothing shows");
    press(&mut state, "p", ALT).expect("the preview panel's key");
    let effects = state.follow_preview();
    (state, effects)
}

#[test]
fn the_panel_follows_the_active_pane_s_cursor_the_list_s_own_too() {
    let (mut state, effects) = with_panel(mixed(), vec![file("x.jpg")]);
    // The cursor the list placed is on the first row: a folder, whose items a worker counts.
    let first = the_preview(&effects);
    assert_eq!(first.path, folder("a").join("d1").expect("a name"));
    assert_eq!(first.kind, EntryKind::Dir);
    assert_eq!(previewed_name(&state), Some("d1"));
    assert!(state.preview().expect("shown").content.is_none());
    // Nothing changed, nothing asked again.
    assert_eq!(state.follow_preview(), []);

    press(&mut state, &special(Key::DownArrow), NONE);
    let second = the_preview(&state.follow_preview());
    assert_eq!(second.path, folder("a").join("a.txt").expect("a name"));
    assert_eq!(second.kind, EntryKind::File);
    assert!(second.generation > first.generation);
    // The answer for the folder came late: it is dropped.
    state.previewed(Previewed {
        generation: first.generation,
        content: PreviewContent::Folder {
            items: 3,
            more: false,
        },
    });
    assert_eq!(state.preview().expect("shown").content, None);
    let text = PreviewContent::Plain;
    state.previewed(Previewed {
        generation: second.generation,
        content: text.clone(),
    });
    let shown = state.preview().expect("shown");
    assert_eq!(shown.content, Some(&text));
    assert_eq!(shown.generation, second.generation);

    // The other pane gets the keys: its cursor's item shows.
    press(&mut state, &special(Key::Tab), NONE);
    let other = the_preview(&state.follow_preview());
    assert_eq!(other.path, folder("b").join("x.jpg").expect("a name"));
    assert_eq!(previewed_name(&state), Some("x.jpg"));
}

#[test]
fn nothing_is_asked_while_neither_the_panel_nor_quick_look_shows() {
    let mut state = ready(mixed(), vec![]);
    press(&mut state, &special(Key::DownArrow), NONE);
    assert_eq!(state.follow_preview(), []);
    assert!(state.preview().is_none());
    // Shown, then hidden while its request is under way: the request stops.
    press(&mut state, "p", ALT);
    the_preview(&state.follow_preview());
    press(&mut state, "p", ALT);
    assert_eq!(state.follow_preview(), [Effect::StopPreview]);
    assert!(state.preview().is_none());
    // Shown again, the item is asked for anew.
    press(&mut state, "p", ALT);
    the_preview(&state.follow_preview());
}

#[test]
fn an_empty_folder_shows_nothing_and_a_link_shows_its_facts_without_a_worker() {
    let (state, effects) = with_panel(vec![], vec![]);
    assert_eq!(effects, []);
    assert!(state.preview().is_none());

    let link = Entry::new("바로 가기", EntryKind::Symlink);
    let (mut state, effects) = with_panel(vec![file("a.txt"), link], vec![]);
    the_preview(&effects);
    // To the link while the file's request is under way: it stops, and the link shows at once.
    press(&mut state, &special(Key::DownArrow), NONE);
    assert_eq!(state.follow_preview(), [Effect::StopPreview]);
    assert_eq!(
        state.preview().expect("shown").content,
        Some(&PreviewContent::Plain)
    );
}

#[test]
fn a_file_that_changes_is_previewed_again() {
    let (mut state, effects) = with_panel(vec![file("log.txt")], vec![]);
    let first = the_preview(&effects);
    let generation = state.panes()[LEFT].active_tab().generation();
    let mut grown = file("log.txt");
    grown.size = 4_096;
    grown.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000));
    state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![Change::Upsert(grown)],
    });
    let again = the_preview(&state.follow_preview());
    assert_eq!(again.path, first.path);
    assert!(again.generation > first.generation);
    assert_eq!(state.preview().expect("shown").entry.size, 4_096);
    // Hidden items shown: a folder counts its items anew.
    let (mut state, effects) = with_panel(vec![dir("d")], vec![]);
    let before = the_preview(&effects);
    press(&mut state, "h", CTRL);
    let after = the_preview(&state.follow_preview());
    assert!(!before.filter.show_hidden && after.filter.show_hidden);
}

#[test]
fn space_opens_quick_look_and_the_arrows_go_from_file_to_file() {
    let mut state = ready(mixed(), vec![]);
    press(&mut state, " ", NONE).expect("Space is quick look");
    assert!(state.quick_look_open());
    // Without the panel too it asks for the item: the folder the list's cursor is on.
    let first = the_preview(&state.follow_preview());
    assert_eq!(first.kind, EntryKind::Dir);

    let step = |state: &mut AppState, key: QuickLookKey| {
        let mut effects = state.quick_look_key(key);
        effects.extend(state.follow_preview());
        (cursor(state, LEFT), effects)
    };
    // → past the folders, each file alone selected, the list scrolled to it.
    let (row, effects) = step(&mut state, QuickLookKey::Next);
    assert_eq!(row, Some(1));
    assert!(effects.contains(&Effect::ScrollTo { side: LEFT, row: 1 }));
    assert_eq!(selected(&state, LEFT), [1]);
    assert_eq!(previewed_name(&state), Some("a.txt"));
    assert_eq!(step(&mut state, QuickLookKey::Next).0, Some(3));
    assert_eq!(step(&mut state, QuickLookKey::Next).0, Some(4));
    // The last file stays.
    let (row, effects) = step(&mut state, QuickLookKey::Next);
    assert_eq!((row, effects), (Some(4), vec![]));
    assert_eq!(step(&mut state, QuickLookKey::Previous).0, Some(3));
    assert_eq!(step(&mut state, QuickLookKey::Previous).0, Some(1));
    // Before the first file there is only a folder.
    assert_eq!(step(&mut state, QuickLookKey::Previous).0, Some(1));
    assert!(state.quick_look_open());

    state.quick_look_key(QuickLookKey::Close);
    assert!(!state.quick_look_open());
    // Without the panel nothing shows any more; what is asked stops.
    assert_eq!(state.follow_preview(), [Effect::StopPreview]);
    // Space opens it again, and Space (the key that opened it, from the menu too) closes it.
    press(&mut state, " ", NONE);
    assert!(state.quick_look_open());
    state.key(Command::QuickLook, NONE, Paging::rows(10), Instant::now());
    assert!(!state.quick_look_open());
}

#[test]
fn quick_look_is_the_same_in_every_view_and_with_one_pane() {
    for mode in ALL_MODES {
        for dual in [true, false] {
            let mut state = ready(mixed(), mixed());
            if !dual {
                press(&mut state, &special(Key::F12), NONE);
            }
            state.set_view_mode(LEFT, mode);
            press(&mut state, " ", NONE);
            assert!(state.quick_look_open(), "{mode:?} {dual}");
            the_preview(&state.follow_preview());
            state.quick_look_key(QuickLookKey::Next);
            state.follow_preview();
            assert_eq!(cursor(&state, LEFT), Some(1), "{mode:?} {dual}");
            assert_eq!(previewed_name(&state), Some("a.txt"), "{mode:?} {dual}");
            state.quick_look_key(QuickLookKey::Close);
            assert!(!state.quick_look_open(), "{mode:?} {dual}");
        }
    }
}

#[test]
fn nothing_to_look_at_opens_nothing() {
    let mut state = ready(vec![], vec![]);
    press(&mut state, " ", NONE);
    assert!(!state.quick_look_open());
    assert_eq!(state.follow_preview(), []);
}

#[test]
fn quick_look_closes_under_a_window_and_when_the_pane_moves_on() {
    let mut state = ready(mixed(), vec![file("x")]);
    press(&mut state, " ", NONE);
    state.follow_preview();
    // A window over the main window (here the pattern window) closes it.
    press(&mut state, "s", CTRL);
    assert!(state.modal().is_some());
    state.follow_preview();
    assert!(!state.quick_look_open());
    state.dialog_cancelled();

    // The tab goes to another folder.
    press(&mut state, " ", NONE);
    state.follow_preview();
    state.go_to(folder("c"));
    state.follow_preview();
    assert!(!state.quick_look_open());

    // The folder empties under it: nothing to show.
    let mut state = ready(vec![file("only")], vec![]);
    press(&mut state, " ", NONE);
    state.follow_preview();
    let generation = state.panes()[LEFT].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![Change::Removed("only".into())],
    });
    state.follow_preview();
    assert!(!state.quick_look_open());
}

#[test]
fn a_space_while_a_name_is_typed_types_and_otherwise_opens_quick_look() {
    let mut state = ready(
        vec![file("my file.txt"), file("my notes.txt"), file("zeta")],
        vec![],
    );
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    // `pane_command` as the controller calls it, with whether a name is being typed then.
    let key = |state: &mut AppState, text: &str, ms: u64| {
        let typing = state.typing(at(ms));
        let command = keys::pane_command(text, NONE, Os::Windows, typing).expect("a key");
        state.key(command, NONE, Paging::rows(10), at(ms));
    };
    key(&mut state, "m", 0);
    key(&mut state, "y", 200);
    assert!(state.typing(at(400)));
    // The space is part of the name: `my n` goes to `my notes.txt`, and nothing opens.
    key(&mut state, " ", 400);
    assert!(!state.quick_look_open());
    key(&mut state, "n", 600);
    assert_eq!(cursor(&state, LEFT), Some(1));
    // After a pause, Space opens quick look.
    assert!(!state.typing(at(1_700)));
    key(&mut state, " ", 1_700);
    assert!(state.quick_look_open());
    state.close_quick_look();

    // Another key ends the name: a Space right after it opens quick look.
    key(&mut state, "z", 2_000);
    assert_eq!(cursor(&state, LEFT), Some(2));
    key(&mut state, &special(Key::UpArrow), 2_100);
    assert!(!state.typing(at(2_200)));
    key(&mut state, " ", 2_200);
    assert!(state.quick_look_open());
}
