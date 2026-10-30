//! The properties window (OPS-06's own window) and its checksums (OPS-10): what it opens for,
//! the requests for its workers and their generations, 중지, the algorithms, the copy of a
//! checksum, and the end of its work when it closes.

use super::*;
use crate::dialogs::Modal;
use crate::hashing::{Algorithm, HashFailure, HashNews, HashRequest};
use crate::properties::{CountStatus, HashStatus, MeasureNews, MeasureRequest, Tally};

const ENTER: Key = Key::Return;

fn sized(name: &str, size: u64) -> Entry {
    let mut entry = file(name);
    entry.size = size;
    entry
}

/// The one request for the measure worker among `effects`.
fn the_measure(effects: &[Effect]) -> MeasureRequest {
    let found: Vec<&MeasureRequest> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Measure(request) => Some(request),
            _ => None,
        })
        .collect();
    match found.as_slice() {
        [request] => (*request).clone(),
        other => panic!("one measure, not {other:?} in {effects:?}"),
    }
}

/// The one request for the checksums among `effects`.
fn the_hash(effects: &[Effect]) -> HashRequest {
    let found: Vec<&HashRequest> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Hash(request) => Some(request),
            _ => None,
        })
        .collect();
    match found.as_slice() {
        [request] => (*request).clone(),
        other => panic!("one hash request, not {other:?} in {effects:?}"),
    }
}

/// Alt+Enter on the left pane, after `choose` chose its rows.
fn open_with(state: &mut AppState, choose: impl FnOnce(&mut AppState)) -> Vec<Effect> {
    choose(state);
    press(state, &special(ENTER), ALT).expect("the properties key")
}

fn statuses(state: &AppState) -> Vec<HashStatus> {
    state
        .properties()
        .expect("the window")
        .hashing
        .rows
        .iter()
        .map(|row| row.status.clone())
        .collect()
}

#[test]
fn alt_enter_opens_the_window_for_the_chosen_items() {
    let mut state = ready(
        vec![dir("docs"), sized("a.txt", 3), sized("b.png", 10)],
        vec![],
    );
    let effects = open_with(&mut state, |state| {
        state.click_row(LEFT, 1, false, false);
        state.click_row(LEFT, 2, true, false);
    });
    assert_eq!(state.modal(), Some(&Modal::Properties));
    let properties = state.properties().expect("the window");
    let names: Vec<String> = (0..properties.items.len())
        .map(|i| properties.name(i))
        .collect();
    assert_eq!(names, ["a.txt", "b.png"]);
    assert!(!properties.own_folder);
    assert_eq!(properties.chosen.bytes, 13);
    // Two files and no folder: nothing for the measure worker, and no checksum before an
    // algorithm is chosen.
    assert_eq!(effects, []);
    assert_eq!(properties.count.status, CountStatus::None);
    assert_eq!(statuses(&state), [HashStatus::Idle, HashStatus::Idle]);
    // While it shows, the panes take no keys.
    assert_eq!(
        press(&mut state, &special(Key::DownArrow), NONE),
        Some(vec![])
    );
    assert_eq!(cursor(&state, LEFT), Some(2));
}

#[test]
fn command_i_opens_it_on_macos_and_closing_it_forgets_it() {
    let mut state = ready(vec![sized("a.txt", 3)], vec![]);
    state.click_row(LEFT, 0, false, false);
    let command = keys::command_for("i", CTRL, Os::MacOs).expect("⌘I");
    assert_eq!(command, Command::Properties);
    state.key(command, CTRL, Paging::rows(10), Instant::now());
    assert_eq!(state.modal(), Some(&Modal::Properties));
    state.dialog_cancelled();
    assert_eq!(state.follow_properties(), []);
    assert!(state.properties().is_none());
}

#[test]
fn with_nothing_chosen_it_opens_for_the_pane_s_folder_which_a_worker_reads_and_counts() {
    let mut state = ready(vec![sized("a.txt", 3)], vec![]);
    // The cursor the list placed is no choice: the window is for the folder itself.
    let effects = press(&mut state, &special(ENTER), ALT).expect("the properties key");
    let request = the_measure(&effects);
    assert_eq!(request.items.len(), 1);
    assert_eq!(request.items[0].path, folder("a"));
    assert_eq!(request.items[0].entry, None);
    assert!(request.link_targets);
    let properties = state.properties().expect("the window");
    assert!(properties.own_folder);
    assert_eq!(properties.count.status, CountStatus::Counting);
    assert_eq!(properties.count.generation, request.generation);
    // The worker reads it, then counts.
    let mut entry = dir("a");
    entry.attributes = Attributes::HIDDEN;
    assert!(state.measure_news(MeasureNews::Stated {
        generation: request.generation,
        item: 0,
        result: Ok(entry.clone()),
    }));
    assert!(state.measure_news(MeasureNews::Counted {
        generation: request.generation,
        tally: Tally {
            files: 5,
            folders: 1,
            bytes: 900,
            ..Tally::default()
        },
    }));
    let tally = Tally {
        files: 7,
        folders: 2,
        bytes: 1_000,
        ..Tally::default()
    };
    assert!(state.measured(request.generation, Some(tally)));
    let properties = state.properties().expect("the window");
    assert_eq!(properties.items[0].entry, Some(entry));
    assert_eq!(properties.chosen.folders, 1);
    assert_eq!(properties.count.status, CountStatus::Done);
    assert_eq!(properties.count.tally, tally);
    assert!(!properties.measuring);
    // No file to hash: no algorithm takes.
    assert_eq!(properties.hashing.rows, []);
    assert_eq!(state.choose_algorithm(0), []);
}

#[test]
fn the_news_of_a_closed_window_is_dropped_and_its_work_stops() {
    let mut state = ready(vec![dir("docs"), sized("a.txt", 3)], vec![]);
    let first = the_measure(&open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    }));
    let hash = the_hash(&{
        state.dialog_cancelled();
        // Closed before an algorithm: only the count stops.
        let stops = state.follow_properties();
        assert_eq!(stops, [Effect::StopMeasure]);
        assert!(state.properties().is_none());
        state.click_row(LEFT, 1, false, false);
        press(&mut state, &special(ENTER), ALT).expect("the properties key");
        state.choose_algorithm(2)
    });
    // The first window's late news has its generation: none of it comes into the new one.
    let late = Tally {
        files: 99,
        ..Tally::default()
    };
    assert!(!state.measure_news(MeasureNews::Counted {
        generation: first.generation,
        tally: late,
    }));
    assert!(!state.measured(first.generation, Some(late)));
    assert_eq!(
        state.properties().expect("the window").count.status,
        CountStatus::None
    );
    // The second window's checksum is under way when it closes: it stops too.
    assert_eq!(hash.algorithm, Algorithm::Sha256);
    assert!(state.hash_news(HashNews::Reading {
        generation: hash.generation,
        row: 0,
        done: 1,
        total: 3,
    }));
    state.dialog_cancelled();
    assert_eq!(state.follow_properties(), [Effect::StopHashes]);
    assert!(!state.hash_news(HashNews::Hashed {
        generation: hash.generation,
        results: vec![(0, Ok("ab".to_owned()))],
    }));
    // Once is enough.
    assert_eq!(state.follow_properties(), []);
}

#[test]
fn another_algorithm_drops_the_checksums_of_the_one_before() {
    let mut state = ready(
        vec![sized("a.txt", 3), sized("b.txt", 4), dir("docs")],
        vec![],
    );
    open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
        state.click_row(LEFT, 1, true, false);
    });
    let md5 = the_hash(&state.choose_algorithm(0));
    assert_eq!(md5.algorithm, Algorithm::Md5);
    let rows: Vec<usize> = md5.files.iter().map(|(row, _)| *row).collect();
    assert_eq!(rows, [0, 1]);
    assert_eq!(statuses(&state), [HashStatus::Waiting, HashStatus::Waiting]);
    assert!(state.hash_news(HashNews::Hashed {
        generation: md5.generation,
        results: vec![(0, Ok("900150983cd24fb0d6963f7d28e17f72".to_owned()))],
    }));
    // The same algorithm again asks for nothing.
    assert_eq!(state.choose_algorithm(0), []);
    // BLAKE3: a new request, under a new generation, which replaces the old one on the worker.
    let blake3 = the_hash(&state.choose_algorithm(3));
    assert!(blake3.generation > md5.generation);
    assert_eq!(statuses(&state), [HashStatus::Waiting, HashStatus::Waiting]);
    // MD5's late news is dropped.
    assert!(!state.hash_news(HashNews::Hashed {
        generation: md5.generation,
        results: vec![(1, Ok("old".to_owned()))],
    }));
    assert!(!state.hashes_failed(md5.generation));
    assert!(state.hash_news(HashNews::Hashed {
        generation: blake3.generation,
        results: vec![(1, Err(HashFailure::NotAFile))],
    }));
    // A row that finished takes nothing more of its request.
    assert!(!state.hash_news(HashNews::Reading {
        generation: blake3.generation,
        row: 1,
        done: 1,
        total: 4,
    }));
    // The worker failed the rest.
    assert!(state.hashes_failed(blake3.generation));
    assert_eq!(
        statuses(&state),
        [
            HashStatus::Failed(HashFailure::Unreadable(ErrorKind::Other)),
            HashStatus::Failed(HashFailure::NotAFile),
        ]
    );
    // Nothing left under way: closing stops nothing.
    state.dialog_cancelled();
    assert_eq!(state.follow_properties(), []);
}

#[test]
fn stopping_the_count_keeps_what_it_found_and_drops_what_comes_after() {
    let mut state = ready(vec![dir("big"), sized("a.txt", 5)], vec![]);
    let request = the_measure(&open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
        state.click_row(LEFT, 1, true, false);
    }));
    assert!(!request.link_targets);
    let found = Tally {
        files: 12_345,
        folders: 678,
        bytes: 1 << 30,
        ..Tally::default()
    };
    assert!(state.measure_news(MeasureNews::Counted {
        generation: request.generation,
        tally: found,
    }));
    assert_eq!(state.stop_count(), [Effect::StopMeasure]);
    // Twice changes nothing.
    assert_eq!(state.stop_count(), []);
    assert!(!state.measure_news(MeasureNews::Counted {
        generation: request.generation,
        tally: Tally {
            files: 20_000,
            ..found
        },
    }));
    assert!(!state.measured(request.generation, Some(found)));
    let properties = state.properties().expect("the window");
    assert_eq!(properties.count.status, CountStatus::Stopped);
    assert_eq!(properties.count.tally, found);
    // Stopped already: closing stops nothing more.
    state.dialog_cancelled();
    assert_eq!(state.follow_properties(), []);
}

#[test]
fn a_count_that_failed_says_so() {
    let mut state = ready(vec![dir("big")], vec![]);
    let request = the_measure(&open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    }));
    assert!(state.measured(request.generation, None));
    assert_eq!(
        state.properties().expect("the window").count.status,
        CountStatus::Failed
    );
}

#[test]
fn a_lone_link_asks_where_it_leads() {
    let mut state = ready(vec![Entry::new("shortcut", EntryKind::Symlink)], vec![]);
    let request = the_measure(&open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    }));
    assert!(request.link_targets);
    assert_eq!(
        state.properties().expect("the window").count.status,
        CountStatus::None
    );
    assert!(state.measure_news(MeasureNews::Linked {
        generation: request.generation,
        item: 0,
        result: Ok("../target".to_owned()),
    }));
    assert!(state.measured(request.generation, Some(Tally::default())));
    let properties = state.properties().expect("the window");
    assert_eq!(properties.items[0].link, Some(Ok("../target".to_owned())));
    // A link is no file to hash.
    assert_eq!(properties.hashing.rows, []);
}

#[test]
fn a_file_stored_online_only_is_not_read() {
    let mut cloud = sized("cloud.mp4", 1 << 30);
    cloud.attributes = Attributes::OFFLINE;
    let mut state = ready(vec![cloud, sized("here.txt", 3)], vec![]);
    open_with(&mut state, |state| {
        state.key(Command::SelectAll, NONE, Paging::rows(10), Instant::now());
    });
    assert_eq!(statuses(&state), [HashStatus::OnlineOnly, HashStatus::Idle]);
    let request = the_hash(&state.choose_algorithm(4));
    let rows: Vec<usize> = request.files.iter().map(|(row, _)| *row).collect();
    assert_eq!(rows, [1]);
    assert_eq!(
        statuses(&state),
        [HashStatus::OnlineOnly, HashStatus::Waiting]
    );
}

#[test]
fn a_checksum_goes_to_the_clipboard_and_says_so() {
    let mut state = ready(vec![sized("a.txt", 3)], vec![]);
    open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    });
    let request = the_hash(&state.choose_algorithm(1));
    // Nothing to copy before it is made.
    assert_eq!(state.copy_checksum(0), []);
    let digest = "a9993e364706816aba3e25717850c26c9cd0d89d";
    state.hash_news(HashNews::Hashed {
        generation: request.generation,
        results: vec![(0, Ok(digest.to_owned()))],
    });
    let effects = state.copy_checksum(0);
    let Some(Effect::Clipboard(ClipboardWork::PutText { token, text })) = effects.first() else {
        panic!("a copy, not {effects:?}");
    };
    assert_eq!(text, digest);
    assert_eq!(state.checksum_copied(*token, Ok(())), []);
    assert_eq!(state.properties().expect("the window").copied, Some(0));
    // A clipboard that would not take it: the window says why (Linux has no text clipboard).
    let effects = state.copy_checksum(0);
    let Some(Effect::Clipboard(ClipboardWork::PutText { token, .. })) = effects.first() else {
        panic!("a copy, not {effects:?}");
    };
    // An answer for another copy is not this one's.
    state.checksum_copied(token + 1, Ok(()));
    state.checksum_copied(*token, Err(ClipboardFailure::Unsupported));
    let properties = state.properties().expect("the window");
    assert_eq!(properties.copied, None);
    assert_eq!(properties.copy_problem, Some(ClipboardFailure::Unsupported));
}

#[test]
fn the_pasted_hash_and_each_change_make_a_new_revision() {
    let mut state = ready(vec![sized("a.txt", 3)], vec![]);
    open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    });
    let revision = |state: &AppState| state.properties().expect("the window").revision;
    let before = revision(&state);
    state.compare_edited("ABC");
    assert_eq!(state.properties().expect("the window").compare, "ABC");
    assert!(revision(&state) > before);
    let same = revision(&state);
    state.compare_edited("ABC");
    assert_eq!(revision(&state), same);
    // Each window has an id of its own.
    let id = state.properties().expect("the window").id;
    state.dialog_cancelled();
    state.follow_properties();
    press(&mut state, &special(ENTER), ALT);
    assert_ne!(state.properties().expect("the window").id, id);
}

#[test]
fn a_window_behind_the_question_whether_to_quit_keeps_its_work_and_the_end_stops_it() {
    let mut state = ready(vec![dir("big")], vec![]);
    let request = the_measure(&open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    }));
    // A job runs, and the window's close button asks first: the properties wait behind.
    state.track(crate::jobs::Started {
        id: JobId(1),
        spec: None,
        work: words::JobWork::Trash,
        origin: None,
        via_history: false,
    });
    state.request_close();
    assert!(matches!(state.modal(), Some(Modal::Confirm(_))));
    assert_eq!(state.follow_properties(), []);
    assert!(state.measure_news(MeasureNews::Counted {
        generation: request.generation,
        tally: Tally {
            files: 1,
            ..Tally::default()
        },
    }));
    // The app ends: the work stops.
    state.confirm_accepted();
    assert_eq!(state.follow_properties(), [Effect::StopMeasure]);
}

#[test]
fn stopping_the_checksums_keeps_those_made_and_the_same_algorithm_makes_the_rest() {
    let mut state = ready(
        vec![
            sized("a.iso", 1 << 32),
            sized("b.iso", 1 << 32),
            sized("c.txt", 3),
        ],
        vec![],
    );
    open_with(&mut state, |state| {
        state.key(Command::SelectAll, NONE, Paging::rows(10), Instant::now());
    });
    // Nothing under way yet: 중지 does nothing.
    assert_eq!(state.stop_hashes(), []);
    let first = the_hash(&state.choose_algorithm(2));
    assert!(state.hash_news(HashNews::Hashed {
        generation: first.generation,
        results: vec![(0, Ok("aa".to_owned()))],
    }));
    assert!(state.hash_news(HashNews::Reading {
        generation: first.generation,
        row: 1,
        done: 1 << 30,
        total: 1 << 32,
    }));
    assert_eq!(state.stop_hashes(), [Effect::StopHashes]);
    assert_eq!(
        statuses(&state),
        [
            HashStatus::Done("aa".to_owned()),
            HashStatus::Stopped,
            HashStatus::Stopped
        ]
    );
    assert!(!state.properties().expect("the window").hashing.under_way());
    // Late news of the stopped request is dropped, and closing now stops nothing more.
    assert!(!state.hash_news(HashNews::Hashed {
        generation: first.generation,
        results: vec![(1, Ok("bb".to_owned()))],
    }));
    assert_eq!(state.stop_hashes(), []);
    // The same algorithm again: the stopped rows alone, under a new generation.
    let rest = the_hash(&state.choose_algorithm(2));
    assert!(rest.generation > first.generation);
    let rows: Vec<usize> = rest.files.iter().map(|(row, _)| *row).collect();
    assert_eq!(rows, [1, 2]);
    assert_eq!(
        statuses(&state),
        [
            HashStatus::Done("aa".to_owned()),
            HashStatus::Waiting,
            HashStatus::Waiting
        ]
    );
    // Several rows in one piece of news.
    assert!(state.hash_news(HashNews::Hashed {
        generation: rest.generation,
        results: vec![(1, Ok("bb".to_owned())), (2, Ok("cc".to_owned()))],
    }));
    assert_eq!(
        statuses(&state),
        ["aa", "bb", "cc"].map(|digest| HashStatus::Done(digest.to_owned()))
    );
    // Nothing stopped: the same algorithm asks for nothing.
    assert_eq!(state.choose_algorithm(2), []);
}

#[test]
fn a_folder_that_cannot_be_listed_is_marked() {
    let mut state = ready(vec![dir("locked")], vec![]);
    let request = the_measure(&open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    }));
    assert!(state.measure_news(MeasureNews::Unlisted {
        generation: request.generation,
        item: 0,
        kind: ErrorKind::PermissionDenied,
    }));
    // Another window's is not this one's.
    assert!(!state.measure_news(MeasureNews::Unlisted {
        generation: request.generation + 1000,
        item: 0,
        kind: ErrorKind::NotFound,
    }));
    assert_eq!(
        state.properties().expect("the window").items[0].unlisted,
        Some(ErrorKind::PermissionDenied)
    );
}

#[test]
fn a_lone_link_whose_worker_failed_says_it_could_not_be_read() {
    let mut state = ready(vec![Entry::new("shortcut", EntryKind::Symlink)], vec![]);
    let request = the_measure(&open_with(&mut state, |state| {
        state.click_row(LEFT, 0, false, false);
    }));
    // The worker panicked before it read the link: no 읽는 중… for ever.
    assert!(state.measured(request.generation, None));
    assert_eq!(
        state.properties().expect("the window").items[0].link,
        Some(Err(ErrorKind::Other))
    );
}

#[test]
fn the_blank_area_s_properties_are_the_folder_s_whatever_is_chosen() {
    let mut state = ready(vec![sized("a.txt", 3), dir("docs")], vec![]);
    state.click_row(LEFT, 0, false, false);
    let request = the_measure(&state.folder_properties());
    assert_eq!(request.items.len(), 1);
    assert_eq!(request.items[0].path, folder("a"));
    let properties = state.properties().expect("the window");
    assert!(properties.own_folder);
    assert_eq!(state.modal(), Some(&Modal::Properties));
    // While a window shows, nothing more opens.
    assert_eq!(state.folder_properties(), []);
    // From the keyboard: Esc leaves nothing chosen, and the key opens the folder's too.
    state.dialog_cancelled();
    state.follow_properties();
    press(&mut state, &special(Key::Escape), NONE);
    press(&mut state, &special(ENTER), ALT).expect("the properties key");
    assert!(state.properties().expect("the window").own_folder);
}
