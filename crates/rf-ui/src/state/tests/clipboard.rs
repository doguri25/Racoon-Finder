//! The system clipboard: what a copy and a cut put there and dim, what a paste submits, when
//! the clipboard is emptied and the dimming goes, Esc, and the text of 경로 복사.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;

use rf_jobs::{
    ConflictPolicy, JobEvent, JobSpec, Outcome, TransferItem, TransferMode, TransferOptions,
    TransferSpec,
};
use rf_platform::clipboard::{ClipboardFiles, DropEffect};

use super::jobs::finish;
use super::*;
use crate::jobs::{JobEffect, Started};
use crate::keys::Command;
use crate::words::{self, JobWork};

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn in_b(name: &str) -> VPath {
    folder("b").join(name).expect("a plain name")
}

fn local(path: &VPath) -> PathBuf {
    path.as_local().expect("a local path").to_path_buf()
}

/// A window whose left pane at `a` holds the folder `docs` and the files `x.txt` and `y.txt`,
/// and whose right pane at `b` holds the file `z.txt`.
fn two_panes() -> AppState {
    ready(
        vec![dir("docs"), file("x.txt"), file("y.txt")],
        vec![file("z.txt")],
    )
}

fn names(list: &[&str]) -> HashSet<OsString> {
    list.iter().map(OsString::from).collect()
}

/// The one request to the clipboard's worker among `effects`.
fn the_work(effects: &[Effect]) -> ClipboardWork {
    let work: Vec<&ClipboardWork> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Clipboard(work) => Some(work),
            _ => None,
        })
        .collect();
    match work.as_slice() {
        [one] => (*one).clone(),
        other => panic!("expected one request, got {other:?} in {effects:?}"),
    }
}

/// The one job `effects` submit, and the pane that asked for it.
fn the_submit(effects: &[Effect]) -> (JobSpec, Option<usize>) {
    let submits: Vec<(JobSpec, Option<usize>)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Job(JobEffect::Submit { spec, origin }) => Some((spec.clone(), *origin)),
            _ => None,
        })
        .collect();
    match submits.as_slice() {
        [one] => one.clone(),
        other => panic!("expected one job, got {other:?} in {effects:?}"),
    }
}

/// Puts row `row` of the left pane on the clipboard with Ctrl+C (`cut` false) or Ctrl+X, and
/// gives the token of the put.
fn put(state: &mut AppState, row: usize, cut: bool) -> u64 {
    state.click_row(LEFT, row, false, false);
    let effects = press(state, if cut { "x" } else { "c" }, CTRL).expect("a command of the window");
    match the_work(&effects) {
        ClipboardWork::PutFiles {
            token, cut: put, ..
        } => {
            assert_eq!(put, cut);
            token
        }
        other => panic!("expected a put, got {other:?}"),
    }
}

/// Ctrl+V in pane `side`, and the token of the read it asks for.
fn paste(state: &mut AppState, side: usize) -> u64 {
    state.activate(side);
    let effects = press(state, "v", CTRL).expect("a command of the window");
    match the_work(&effects) {
        ClipboardWork::ReadFiles { token } => token,
        other => panic!("expected a read, got {other:?}"),
    }
}

fn files(paths: &[VPath], effect: Option<DropEffect>, change: u64) -> ClipboardFiles {
    ClipboardFiles {
        paths: paths.iter().map(local).collect(),
        effect,
        change,
        skipped: 0,
    }
}

fn auto_rename() -> TransferOptions {
    TransferOptions {
        conflict: ConflictPolicy::AutoRename,
        ..TransferOptions::default()
    }
}

fn transfer(mode: TransferMode, items: &[(VPath, VPath)]) -> JobSpec {
    JobSpec::Transfer(TransferSpec {
        mode,
        items: items
            .iter()
            .map(|(source, target)| TransferItem {
                source: source.clone(),
                target: target.clone(),
            })
            .collect(),
        options: auto_rename(),
    })
}

#[test]
fn a_cut_dims_its_items_until_the_clipboard_holds_something_else() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    assert_eq!(
        the_work(&press(&mut state, "x", CTRL).expect("a command")),
        ClipboardWork::PutFiles {
            token: token + 1,
            paths: vec![in_a("x.txt")],
            cut: true
        }
    );
    // Dimmed at once, in the pane that shows their folder only.
    assert_eq!(state.cut_names(&folder("a")), names(&["x.txt"]));
    assert_eq!(state.cut_names(&folder("b")), names(&[]));
    // Nothing to watch while the put is on its way.
    assert!(!state.clipboard_watched());
    assert_eq!(state.clipboard_put(token + 1, Ok(7)), []);
    assert!(state.clipboard_watched());
    // The same count: still dimmed. A count the system does not tell changes nothing.
    assert_eq!(state.clipboard_polled(Some(7)), None);
    assert_eq!(state.clipboard_polled(None), None);
    assert_eq!(state.cut_names(&folder("a")), names(&["x.txt"]));
    // Another app put something there: the dimming goes, and the watch with it.
    assert_eq!(state.clipboard_polled(Some(8)), Some(vec![]));
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    assert!(!state.clipboard_watched());
}

#[test]
fn a_copy_dims_nothing_and_ends_a_cut() {
    let mut state = two_panes();
    let cut = put(&mut state, 1, true);
    state.clipboard_put(cut, Ok(3));
    let copy = put(&mut state, 2, false);
    assert_ne!(copy, cut);
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    assert!(!state.clipboard_watched());
    // The late answer of the cut does not bring it back.
    assert_eq!(state.clipboard_put(cut, Ok(3)), []);
    assert_eq!(state.clipboard_put(copy, Ok(4)), []);
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    assert!(!state.clipboard_watched());
}

#[test]
fn a_put_that_fails_says_why_and_dims_nothing() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    assert_eq!(
        state.clipboard_put(token, Err(ClipboardFailure::Unavailable)),
        [Effect::Notice(
            words::clipboard_failed(ClipboardFailure::Unavailable).to_owned()
        )]
    );
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    assert!(!state.clipboard_watched());
}

#[test]
fn nothing_goes_on_the_clipboard_from_a_row_the_user_did_not_choose() {
    // The list put the cursor on its first row for the keys: no command acts on it.
    let mut state = two_panes();
    for key in ["c", "x"] {
        assert_eq!(press(&mut state, key, CTRL), Some(vec![]), "{key}");
    }
    assert_eq!(press(&mut state, "C", CTRL_SHIFT), Some(vec![]));
    // An empty folder has nothing at all.
    let mut empty = ready(vec![], vec![]);
    assert_eq!(press(&mut empty, "c", CTRL), Some(vec![]));
}

#[test]
fn a_paste_copies_what_the_clipboard_holds_and_numbers_names_that_are_taken() {
    let mut state = two_panes();
    // Files another app copied (File Explorer: a copy says copy).
    let token = paste(&mut state, LEFT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(
            &[in_b("z.txt"), in_b("x.txt")],
            Some(DropEffect::Copy),
            12,
        ))),
    );
    let (spec, origin) = the_submit(&effects);
    assert_eq!(origin, Some(LEFT));
    assert_eq!(
        spec,
        transfer(
            TransferMode::Copy,
            &[
                (in_b("z.txt"), in_a("z.txt")),
                // `x.txt` is in `a` already: the job names it `x (2).txt` (DND-11).
                (in_b("x.txt"), in_a("x.txt")),
            ]
        )
    );
    // A copy into the folder the item is in makes a numbered copy of it too.
    let token = put(&mut state, 1, false);
    state.clipboard_put(token, Ok(13));
    let token = paste(&mut state, LEFT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Copy), 13))),
    );
    assert_eq!(
        the_submit(&effects).0,
        transfer(TransferMode::Copy, &[(in_a("x.txt"), in_a("x.txt"))])
    );
    // A copy leaves the clipboard as it is, whatever its job does.
    let (spec, _) = the_submit(&effects);
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |_| {});
    assert!(!effects.iter().any(|e| matches!(e, Effect::Clipboard(_))));
}

#[test]
fn a_cut_pastes_as_a_move_and_the_clipboard_empties_once_everything_moved() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(5));
    let token = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 5))),
    );
    let (spec, origin) = the_submit(&effects);
    assert_eq!(origin, Some(RIGHT));
    assert_eq!(
        spec,
        transfer(TransferMode::Move, &[(in_a("x.txt"), in_b("x.txt"))])
    );
    // A cut deletes nothing itself: the items stay dimmed while their move runs.
    assert_eq!(state.cut_names(&folder("a")), names(&["x.txt"]));
    // A move that failed for an item leaves the clipboard and the dimming as they are.
    let effects = finish(&mut state, 1, &spec, JobWork::Move, |report| {
        report.failures.push(rf_jobs::ItemFailure {
            source: Some(in_a("x.txt")),
            target: Some(in_b("x.txt")),
            stage: rf_jobs::Stage::Commit,
            kind: ErrorKind::PermissionDenied,
            message: String::new(),
            reason: None,
        });
    });
    assert!(!effects.iter().any(|e| matches!(e, Effect::Clipboard(_))));
    assert_eq!(state.cut_names(&folder("a")), names(&["x.txt"]));
    // Pasted again, it moves everything: the clipboard is emptied if it still holds the cut,
    // and nothing shows dimmed any more.
    let token = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 5))),
    );
    let (spec, _) = the_submit(&effects);
    let effects = finish(&mut state, 2, &spec, JobWork::Move, |_| {});
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Clipboard(_)))
            .collect::<Vec<_>>(),
        [&Effect::Clipboard(ClipboardWork::ClearIf(5))]
    );
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    assert!(!state.clipboard_watched());
}

#[test]
fn a_move_that_copied_instead_keeps_the_clipboard() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(5));
    let token = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 5))),
    );
    let (spec, _) = the_submit(&effects);
    // The folder of the source could not be written to: the job copied, and the source stays.
    let effects = finish(&mut state, 1, &spec, JobWork::Move, |report| {
        report.copied_instead_of_moved = true;
    });
    assert!(!effects.iter().any(|e| matches!(e, Effect::Clipboard(_))));
    // Cancelled, the same.
    let token = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 5))),
    );
    let (spec, _) = the_submit(&effects);
    let effects = finish(&mut state, 2, &spec, JobWork::Move, |report| {
        report.outcome = Outcome::Cancelled;
    });
    assert!(!effects.iter().any(|e| matches!(e, Effect::Clipboard(_))));
}

#[test]
fn the_same_cut_pasted_twice_moves_once() {
    let mut state = two_panes();
    let first = paste(&mut state, RIGHT);
    let second = paste(&mut state, RIGHT);
    let cut = || Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 9)));
    the_submit(&state.clipboard_read(first, cut()));
    assert_eq!(
        state.clipboard_read(second, cut()),
        [Effect::Notice(words::paste_moving().to_owned())]
    );
    // A copy may be pasted as often as asked.
    let first = paste(&mut state, RIGHT);
    let second = paste(&mut state, RIGHT);
    let copy = || Ok(Some(files(&[in_a("y.txt")], Some(DropEffect::Copy), 10)));
    the_submit(&state.clipboard_read(first, copy()));
    the_submit(&state.clipboard_read(second, copy()));
}

#[test]
fn without_a_drop_effect_the_window_s_own_cut_moves_and_another_app_s_files_copy() {
    // macOS: the pasteboard does not say whether to move.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(20));
    let token = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(token, Ok(Some(files(&[in_a("x.txt")], None, 20))));
    assert_eq!(transfer_mode(&the_submit(&effects).0), TransferMode::Move);
    // Another app has put its files there since: they are the truth, copied, and the cut ends.
    let token = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(token, Ok(Some(files(&[in_a("y.txt")], None, 21))));
    assert_eq!(transfer_mode(&the_submit(&effects).0), TransferMode::Copy);
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    // Finder's ⌥⌘V moves what it pastes, whatever the clipboard says.
    state.activate(RIGHT);
    let effects = state
        .key(Command::PasteMove, NONE, Paging::rows(10), Instant::now())
        .expect("a command of the window");
    let ClipboardWork::ReadFiles { token } = the_work(&effects) else {
        panic!("expected a read");
    };
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_a("y.txt")], Some(DropEffect::Copy), 22))),
    );
    assert_eq!(transfer_mode(&the_submit(&effects).0), TransferMode::Move);
}

fn transfer_mode(spec: &JobSpec) -> TransferMode {
    match spec {
        JobSpec::Transfer(transfer) => transfer.mode,
        other => panic!("expected a transfer, got {other:?}"),
    }
}

#[test]
fn a_paste_never_puts_a_folder_into_itself_nor_moves_items_where_they_are() {
    let mut state = two_panes();
    // The folder `docs`, cut and pasted into itself from its own row's menu.
    let token = put(&mut state, 0, true);
    state.clipboard_put(token, Ok(30));
    state.row_menu(LEFT, 0);
    let ClipboardWork::ReadFiles { token } = the_work(&state.paste_from_menu(false)) else {
        panic!("expected a read");
    };
    assert_eq!(
        state.clipboard_read(
            token,
            Ok(Some(files(&[in_a("docs")], Some(DropEffect::Move), 30)))
        ),
        [Effect::Notice(words::paste_into_itself().to_owned())]
    );
    // Copied into itself, the same.
    let token = paste(&mut state, LEFT);
    state.row_menu(LEFT, 0);
    let ClipboardWork::ReadFiles { token: into_docs } = the_work(&state.paste_from_menu(false))
    else {
        panic!("expected a read");
    };
    assert_eq!(
        state.clipboard_read(
            into_docs,
            Ok(Some(files(&[in_a("docs")], Some(DropEffect::Copy), 31)))
        ),
        [Effect::Notice(words::paste_into_itself().to_owned())]
    );
    // Cut and pasted into its own folder, nothing moves (DND-09).
    assert_eq!(
        state.clipboard_read(
            token,
            Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 32)))
        ),
        [Effect::Notice(words::paste_already_here().to_owned())]
    );
    // With items from elsewhere, only those move.
    let token = paste(&mut state, LEFT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(
            &[in_a("x.txt"), in_b("z.txt")],
            Some(DropEffect::Move),
            33,
        ))),
    );
    assert_eq!(
        the_submit(&effects).0,
        transfer(TransferMode::Move, &[(in_b("z.txt"), in_a("z.txt"))])
    );
}

#[test]
fn the_row_menu_pastes_into_the_folder_the_user_chose() {
    let mut state = two_panes();
    let read_folder = |state: &mut AppState| -> u64 {
        match the_work(&state.paste_from_menu(false)) {
            ClipboardWork::ReadFiles { token } => token,
            other => panic!("expected a read, got {other:?}"),
        }
    };
    let pasted_to = |state: &mut AppState, token: u64| -> VPath {
        let effects = state.clipboard_read(
            token,
            Ok(Some(files(&[in_b("z.txt")], Some(DropEffect::Copy), 1))),
        );
        match the_submit(&effects).0 {
            JobSpec::Transfer(spec) => spec.items[0].target.parent().expect("a folder"),
            other => panic!("expected a transfer, got {other:?}"),
        }
    };
    // The cursor the list placed on `docs` is not the user's: into the pane's folder.
    let token = read_folder(&mut state);
    assert_eq!(pasted_to(&mut state, token), folder("a"));
    // The menu of the folder `docs`: into it.
    state.row_menu(LEFT, 0);
    let token = read_folder(&mut state);
    assert_eq!(pasted_to(&mut state, token), in_a("docs"));
    // The menu of a file: into the pane's folder.
    state.row_menu(LEFT, 1);
    let token = read_folder(&mut state);
    assert_eq!(pasted_to(&mut state, token), folder("a"));
    // Ctrl+V with the folder chosen pastes into the pane's folder, as File Explorer does.
    state.click_row(LEFT, 0, false, false);
    let token = paste(&mut state, LEFT);
    assert_eq!(pasted_to(&mut state, token), folder("a"));
}

#[test]
fn nothing_to_paste_and_a_clipboard_that_cannot_be_read_say_so() {
    let mut state = two_panes();
    let token = paste(&mut state, LEFT);
    assert_eq!(
        state.clipboard_read(token, Ok(None)),
        [Effect::Notice(words::nothing_to_paste().to_owned())]
    );
    let token = paste(&mut state, LEFT);
    assert_eq!(
        state.clipboard_read(token, Err(ClipboardFailure::Unavailable)),
        [Effect::Notice(
            words::clipboard_failed(ClipboardFailure::Unavailable).to_owned()
        )]
    );
    // Paths that are not local ones are not pasted, and the notice says why.
    let token = paste(&mut state, LEFT);
    let relative = ClipboardFiles {
        paths: vec![PathBuf::from("relative.txt")],
        effect: Some(DropEffect::Copy),
        change: 2,
        skipped: 0,
    };
    assert_eq!(
        state.clipboard_read(token, Ok(Some(relative))),
        [Effect::Notice(words::clipboard_skipped(1))]
    );
    // An answer nobody asked for, or asked for twice, does nothing.
    assert_eq!(state.clipboard_read(token, Ok(None)), []);
    assert_eq!(state.clipboard_read(999, Ok(None)), []);
}

/// Esc alone in the panes (`keys::cancels_cut`).
fn esc(state: &mut AppState) -> Option<Vec<Effect>> {
    state.escape(Paging::rows(10), Instant::now())
}

#[test]
fn esc_clears_the_selection_first_and_then_cancels_the_cut() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(40));
    // The selection goes first; the items stay cut.
    assert_eq!(esc(&mut state), Some(vec![]));
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    assert_eq!(state.cut_names(&folder("a")), names(&["x.txt"]));
    // Then the cut: no longer dimmed, and the clipboard emptied if it still holds them, so
    // that no later paste moves them.
    assert_eq!(
        esc(&mut state),
        Some(vec![Effect::Clipboard(ClipboardWork::ClearIf(40))])
    );
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    // Until the emptying is seen to work, the window watches the clipboard.
    assert!(state.clipboard_watched());
    assert_eq!(state.clipboard_cleared(40, Ok(true)), []);
    assert!(!state.clipboard_watched());
    // Then nothing.
    assert_eq!(esc(&mut state), Some(vec![]));
    // A copy is no cut to cancel.
    let token = put(&mut state, 2, false);
    state.clipboard_put(token, Ok(41));
    esc(&mut state);
    assert_eq!(esc(&mut state), Some(vec![]));
}

#[test]
fn only_esc_cancels_a_cut_not_the_menu_or_the_other_key() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(42));
    esc(&mut state);
    // The 선택 menu's 선택 해제, and macOS's ⌥⌘A, are the command alone: the cut stays.
    let clear = |state: &mut AppState| {
        state.key(
            Command::ClearSelection,
            Mods::NONE,
            Paging::rows(10),
            Instant::now(),
        )
    };
    assert_eq!(clear(&mut state), Some(vec![]));
    assert_eq!(state.cut_names(&folder("a")), names(&["x.txt"]));
    assert!(state.clipboard_watched());
    // A dialog over the window takes Esc itself; one that comes here does nothing.
    press(&mut state, "N", CTRL_SHIFT);
    assert_eq!(esc(&mut state), Some(vec![]));
    assert_eq!(state.cut_names(&folder("a")), names(&["x.txt"]));
}

#[test]
fn esc_before_the_cut_is_on_the_clipboard_empties_it_once_it_is() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    esc(&mut state);
    assert_eq!(esc(&mut state), Some(vec![]));
    assert_eq!(state.cut_names(&folder("a")), names(&[]));
    // The put tells its count: the clipboard is emptied if nothing came since.
    assert_eq!(
        state.clipboard_put(token, Ok(50)),
        [Effect::Clipboard(ClipboardWork::ClearIf(50))]
    );
    assert_eq!(state.clipboard_cleared(50, Ok(true)), []);
    assert!(!state.clipboard_watched());
    // A put that failed has nothing to empty.
    let token = put(&mut state, 1, true);
    esc(&mut state);
    esc(&mut state);
    assert_eq!(
        state.clipboard_put(token, Err(ClipboardFailure::Unavailable)),
        []
    );
    assert!(!state.clipboard_watched());
}

#[test]
fn a_cut_cancelled_before_its_put_was_told_is_never_pasted() {
    // Ctrl+X, Esc, Esc, Ctrl+V, quicker than the put: the read is queued before the emptying.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    esc(&mut state);
    esc(&mut state);
    let read = paste(&mut state, RIGHT);
    assert_eq!(
        state.clipboard_put(token, Ok(60)),
        [Effect::Clipboard(ClipboardWork::ClearIf(60))]
    );
    // The read finds the cut, still a move on the clipboard: nothing is pasted, and it is
    // asked to go again.
    let cut = || Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 60)));
    assert_eq!(
        state.clipboard_read(read, cut()),
        [
            Effect::Notice(words::cut_cancelled().to_owned()),
            Effect::Clipboard(ClipboardWork::ClearIf(60)),
        ]
    );
    // Nor later, while the clipboard still holds it (its emptying failed).
    assert_eq!(
        state.clipboard_cleared(60, Err(ClipboardFailure::Unavailable)),
        []
    );
    let read = paste(&mut state, RIGHT);
    assert!(
        !state
            .clipboard_read(read, cut())
            .iter()
            .any(|e| matches!(e, Effect::Job(_)))
    );
    // Finder's ⌥⌘V asks for a move by name: it moves.
    state.activate(RIGHT);
    let effects = state
        .key(Command::PasteMove, NONE, Paging::rows(10), Instant::now())
        .expect("a command of the window");
    let ClipboardWork::ReadFiles { token } = the_work(&effects) else {
        panic!("expected a read");
    };
    let effects = state.clipboard_read(token, Ok(Some(files(&[in_a("x.txt")], None, 60))));
    assert_eq!(transfer_mode(&the_submit(&effects).0), TransferMode::Move);
    // Another app's content, or the window's next put, is pasted as ever.
    let read = paste(&mut state, RIGHT);
    let other = Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 61)));
    the_submit(&state.clipboard_read(read, other));
    let token = put(&mut state, 2, true);
    state.clipboard_put(token, Ok(62));
    let read = paste(&mut state, RIGHT);
    let cut = Ok(Some(files(&[in_a("y.txt")], Some(DropEffect::Move), 62)));
    assert_eq!(
        transfer_mode(&the_submit(&state.clipboard_read(read, cut)).0),
        TransferMode::Move
    );
}

#[test]
fn a_cancelled_cut_the_clipboard_will_not_drop_is_tried_again_and_then_told() {
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(70));
    esc(&mut state);
    assert_eq!(
        esc(&mut state),
        Some(vec![Effect::Clipboard(ClipboardWork::ClearIf(70))])
    );
    let busy = || Err(ClipboardFailure::Unavailable);
    // Each failure waits for the next look at the change count, which tries again while the
    // clipboard still holds the cut.
    for _ in 1..CLEAR_TRIES {
        assert_eq!(state.clipboard_cleared(70, busy()), []);
        assert!(state.clipboard_watched());
        assert_eq!(
            state.clipboard_polled(Some(70)),
            Some(vec![Effect::Clipboard(ClipboardWork::ClearIf(70))])
        );
    }
    // The last failure tells the user that a paste elsewhere could still move the items.
    assert_eq!(
        state.clipboard_cleared(70, busy()),
        [Effect::Notice(words::clipboard_not_emptied().to_owned())]
    );
    assert!(!state.clipboard_watched());
    assert_eq!(state.clipboard_polled(Some(70)), None);
    // The window's own paste still pastes nothing of it.
    let read = paste(&mut state, RIGHT);
    let cut = Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 70)));
    assert_eq!(
        state.clipboard_read(read, cut)[0],
        Effect::Notice(words::cut_cancelled().to_owned())
    );

    // Once something else is on the clipboard, nothing is left to empty.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(80));
    esc(&mut state);
    esc(&mut state);
    assert_eq!(state.clipboard_cleared(80, busy()), []);
    assert_eq!(state.clipboard_polled(Some(81)), None);
    assert!(!state.clipboard_watched());
    // An answer for another emptying (after a move) changes nothing.
    assert_eq!(state.clipboard_cleared(80, busy()), []);
    assert_eq!(state.clipboard_cleared(99, busy()), []);
}

#[test]
fn a_failed_put_after_a_cancelled_cut_keeps_the_cut_guarded() {
    let busy = || Err(ClipboardFailure::Unavailable);
    let cut = || Ok(Some(files(&[in_a("x.txt")], Some(DropEffect::Move), 100)));
    let no_move = |effects: &[Effect]| {
        !effects.iter().any(|effect| {
            matches!(effect, Effect::Job(JobEffect::Submit { spec, .. })
                if transfer_mode(spec) == TransferMode::Move)
        })
    };
    // Ctrl+X, Esc, Esc (the emptying fails), Ctrl+C (the put fails too): the clipboard still
    // holds the cut.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(100));
    esc(&mut state);
    esc(&mut state);
    assert_eq!(state.clipboard_cleared(100, busy()), []);
    let copy = put(&mut state, 2, false);
    assert_eq!(
        state.clipboard_put(copy, Err(ClipboardFailure::Unavailable)),
        [Effect::Notice(
            words::clipboard_failed(ClipboardFailure::Unavailable).to_owned()
        )]
    );
    // A paste of it moves nothing, and asks for the emptying again.
    let read = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(read, cut());
    assert!(no_move(&effects), "{effects:?}");
    assert!(effects.contains(&Effect::Clipboard(ClipboardWork::ClearIf(100))));
    // The emptying is still tried, and the warning still comes once it keeps failing.
    assert!(state.clipboard_watched());
    for _ in 2..CLEAR_TRIES {
        assert_eq!(
            state.clipboard_polled(Some(100)),
            Some(vec![Effect::Clipboard(ClipboardWork::ClearIf(100))])
        );
        assert_eq!(state.clipboard_cleared(100, busy()), []);
    }
    assert_eq!(
        state.clipboard_cleared(100, busy()),
        [Effect::Notice(words::clipboard_not_emptied().to_owned())]
    );

    // The same with the cut's put still on its way when Esc came: its count is guarded once
    // it is told, whatever the later put does.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    esc(&mut state);
    esc(&mut state);
    let copy = put(&mut state, 2, false);
    assert_eq!(
        state.clipboard_put(token, Ok(100)),
        [Effect::Clipboard(ClipboardWork::ClearIf(100))]
    );
    state.clipboard_put(copy, Err(ClipboardFailure::Unavailable));
    let read = paste(&mut state, RIGHT);
    assert!(no_move(&state.clipboard_read(read, cut())));
    assert!(state.clipboard_watched());

    // A later put that works replaces the cut: nothing is left to empty or to warn about.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(100));
    esc(&mut state);
    esc(&mut state);
    assert_eq!(state.clipboard_cleared(100, busy()), []);
    let copy = put(&mut state, 2, false);
    assert_eq!(state.clipboard_put(copy, Ok(101)), []);
    assert!(!state.clipboard_watched());
    assert_eq!(state.clipboard_cleared(100, busy()), []);
}

#[test]
fn a_device_path_on_the_clipboard_is_left_out_and_told() {
    // On Windows a volume shadow copy's path stays a verbatim one; elsewhere it is not even an
    // absolute path. Either way it is no place to paste from.
    let mut state = two_panes();
    let read = paste(&mut state, LEFT);
    let shadow = PathBuf::from(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\a.txt");
    let found = ClipboardFiles {
        paths: vec![shadow.clone(), local(&in_b("z.txt"))],
        effect: Some(DropEffect::Copy),
        change: 90,
        skipped: 0,
    };
    let effects = state.clipboard_read(read, Ok(Some(found)));
    assert_eq!(
        the_submit(&effects).0,
        transfer(TransferMode::Copy, &[(in_b("z.txt"), in_a("z.txt"))])
    );
    assert!(effects.contains(&Effect::Notice(words::paste_left_out(1))));
    // Nothing but such paths: nothing is pasted, and the notice says why.
    let read = paste(&mut state, LEFT);
    let only = ClipboardFiles {
        paths: vec![shadow.clone()],
        effect: Some(DropEffect::Copy),
        change: 91,
        skipped: 0,
    };
    assert_eq!(
        state.clipboard_read(read, Ok(Some(only))),
        [Effect::Notice(words::clipboard_skipped(1))]
    );
}

#[test]
fn file_urls_the_system_left_out_are_told_with_the_paths_left_out() {
    // macOS: two file URLs that name no path of this computer (another computer's, an encoded
    // `/`), which the system's reading left out, and a device path that is no local one.
    let mut state = two_panes();
    let read = paste(&mut state, LEFT);
    let shadow = PathBuf::from(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\a.txt");
    let found = ClipboardFiles {
        paths: vec![local(&in_b("z.txt")), shadow],
        effect: None,
        change: 92,
        skipped: 2,
    };
    let effects = state.clipboard_read(read, Ok(Some(found)));
    assert_eq!(
        the_submit(&effects).0,
        transfer(TransferMode::Copy, &[(in_b("z.txt"), in_a("z.txt"))])
    );
    assert!(effects.contains(&Effect::Notice(words::paste_left_out(3))));
    // The left-out URLs alone, with no path: nothing is pasted, and the notice says why.
    let read = paste(&mut state, LEFT);
    let none = ClipboardFiles {
        paths: Vec::new(),
        effect: None,
        change: 93,
        skipped: 2,
    };
    assert_eq!(
        state.clipboard_read(read, Ok(Some(none))),
        [Effect::Notice(words::clipboard_skipped(2))]
    );
}

#[test]
fn the_clipboard_keys_do_nothing_while_a_dialog_shows() {
    let mut state = two_panes();
    state.click_row(LEFT, 1, false, false);
    press(&mut state, "N", CTRL_SHIFT);
    assert!(state.modal().is_some());
    for (key, mods) in [("c", CTRL), ("x", CTRL), ("v", CTRL), ("C", CTRL_SHIFT)] {
        assert_eq!(press(&mut state, key, mods), Some(vec![]), "{key}");
    }
    assert_eq!(state.paste_from_menu(false), []);
    assert_eq!(state.copy_paths_as(PathForm::Name), []);
}

#[test]
fn copying_paths_puts_the_items_in_the_form_asked() {
    let mut state = two_panes();
    state.click_row(LEFT, 1, false, false);
    state.click_row(LEFT, 2, true, false);
    // The key's form is the system's: quoted on Windows, as they are on macOS.
    assert_eq!(
        press(&mut state, "C", CTRL_SHIFT),
        Some(vec![Effect::Clipboard(ClipboardWork::PutPaths {
            paths: vec![in_a("x.txt"), in_a("y.txt")],
            form: PathForm::of_key(Os::CURRENT),
        })])
    );
    for form in PathForm::ALL {
        assert_eq!(
            state.copy_paths_as(form),
            [Effect::Clipboard(ClipboardWork::PutPaths {
                paths: vec![in_a("x.txt"), in_a("y.txt")],
                form,
            })]
        );
    }
    // How it went: only a failure says anything.
    assert_eq!(state.paths_copied(Ok(())), []);
    assert_eq!(
        state.paths_copied(Err(ClipboardFailure::Unsupported)),
        [Effect::Notice(
            words::clipboard_failed(ClipboardFailure::Unsupported).to_owned()
        )]
    );
}

#[test]
fn the_key_copies_paths_as_its_system_does() {
    assert_eq!(PathForm::of_key(Os::Windows), PathForm::Quoted);
    assert_eq!(PathForm::of_key(Os::Other), PathForm::Quoted);
    assert_eq!(PathForm::of_key(Os::MacOs), PathForm::Full);
    for (i, form) in PathForm::ALL.into_iter().enumerate() {
        assert_eq!(usize::try_from(form.number()).ok(), Some(i));
        assert_eq!(PathForm::from_number(form.number()), Some(form));
    }
    assert_eq!(PathForm::from_number(6), None);
    assert_eq!(PathForm::from_number(-1), None);
}

#[test]
fn each_form_of_a_path_follows_its_system() {
    let windows = ["C:\\문서\\보고서 (최종).txt".to_owned()];
    let check = |paths: &[String], os: Os, cases: &[(PathForm, &str)]| {
        for (form, wanted) in cases {
            assert_eq!(path_text(paths, *form, os), *wanted, "{os:?} {form:?}");
        }
    };
    check(
        &windows,
        Os::Windows,
        &[
            (PathForm::Full, "C:\\문서\\보고서 (최종).txt"),
            (PathForm::Name, "보고서 (최종).txt"),
            (PathForm::Quoted, "\"C:\\문서\\보고서 (최종).txt\""),
            // The worker has put a share in the place of a mapped drive already.
            (PathForm::Unc, "C:\\문서\\보고서 (최종).txt"),
            (PathForm::Slashes, "C:/문서/보고서 (최종).txt"),
            (
                PathForm::FileUrl,
                "file:///C:/%EB%AC%B8%EC%84%9C/%EB%B3%B4%EA%B3%A0%EC%84%9C%20(%EC%B5%9C%EC%A2%85).txt",
            ),
        ],
    );
    let share = ["\\\\server\\share\\a b.txt".to_owned()];
    check(
        &share,
        Os::Windows,
        &[
            (PathForm::Name, "a b.txt"),
            (PathForm::Slashes, "//server/share/a b.txt"),
            (PathForm::FileUrl, "file://server/share/a%20b.txt"),
        ],
    );
    let unix = ["/Users/me/보고서 \"$HOME\" `x`.txt".to_owned()];
    for os in [Os::MacOs, Os::Other] {
        check(
            &unix,
            os,
            &[
                (PathForm::Full, "/Users/me/보고서 \"$HOME\" `x`.txt"),
                (PathForm::Name, "보고서 \"$HOME\" `x`.txt"),
                // A word a POSIX shell reads back as the path: nothing in single quotes is read
                // specially.
                (PathForm::Quoted, "'/Users/me/보고서 \"$HOME\" `x`.txt'"),
                (PathForm::Unc, "/Users/me/보고서 \"$HOME\" `x`.txt"),
                (PathForm::Slashes, "/Users/me/보고서 \"$HOME\" `x`.txt"),
                (
                    PathForm::FileUrl,
                    "file:///Users/me/%EB%B3%B4%EA%B3%A0%EC%84%9C%20%22$HOME%22%20%60x%60.txt",
                ),
            ],
        );
    }
    // A quote of the name closes the quotes, goes escaped, and opens them again; a line break
    // stays inside them.
    for os in [Os::MacOs, Os::Other] {
        assert_eq!(
            path_text(&["/x/it's \\ 'q'.txt".to_owned()], PathForm::Quoted, os),
            "'/x/it'\\''s \\ '\\''q'\\''.txt'"
        );
        assert_eq!(
            path_text(&["/x/a\nb".to_owned()], PathForm::Quoted, os),
            "'/x/a\nb'"
        );
        assert_eq!(
            path_text(&["/x/a\nb".to_owned()], PathForm::FileUrl, os),
            "file:///x/a%0Ab"
        );
    }
    // A root has no name: it is its own.
    assert_eq!(
        path_text(&["C:\\".to_owned()], PathForm::Name, Os::Windows),
        "C:\\"
    );
    assert_eq!(path_text(&["/".to_owned()], PathForm::Name, Os::MacOs), "/");
}

#[test]
fn several_paths_take_a_line_each() {
    let paths = ["C:\\a".to_owned(), "C:\\b c".to_owned()];
    assert_eq!(
        path_text(&paths, PathForm::Quoted, Os::Windows),
        "\"C:\\a\"\r\n\"C:\\b c\""
    );
    let paths = ["/a".to_owned(), "/b c".to_owned()];
    assert_eq!(path_text(&paths, PathForm::Name, Os::MacOs), "a\nb c");
    assert_eq!(path_text(&paths, PathForm::Full, Os::Other), "/a\n/b c");
    assert_eq!(path_text(&[], PathForm::Full, Os::Windows), "");
}

#[test]
fn file_urls_read_back_as_their_paths() {
    let windows = [
        "C:\\",
        "C:\\Program Files\\a.txt",
        "D:\\문서\\100% 완료 #1?.txt",
        "C:\\a+b;c=d@e!f$g&h'(i)~j,k.txt",
        "C:\\[x]{y}^z`|.txt",
        "\\\\server\\share\\",
        "\\\\server\\share\\폴더\\a b.txt",
    ];
    for path in windows {
        let url = file_url(path, Os::Windows);
        assert!(url.is_ascii(), "{url}");
        assert!(!url.contains(' '), "{url}");
        assert_eq!(
            read_file_url(&url, Os::Windows),
            Some(FileUrl::Path(path.to_owned())),
            "{url}"
        );
    }
    let unix = [
        "/",
        "/Users/me/a b.txt",
        "/tmp/100% 완료 #1?.txt",
        "/x/a:b@c;d=e.txt",
        "/x/\"q\" <a> {b} `c` ^|.txt",
        "/x/한글 폴더/끝",
    ];
    for os in [Os::MacOs, Os::Other] {
        for path in unix {
            let url = file_url(path, os);
            assert!(url.is_ascii(), "{url}");
            assert_eq!(
                read_file_url(&url, os),
                Some(FileUrl::Path(path.to_owned())),
                "{url}"
            );
        }
    }
    // A backslash in a Unix name is encoded, as a URL must, and reads back as a character of
    // the name; on Windows it is a separator, which no name holds.
    for os in [Os::MacOs, Os::Other] {
        let url = file_url("/a\\b", os);
        assert_eq!(url, "file:///a%5Cb");
        assert_eq!(
            read_file_url(&url, os),
            Some(FileUrl::Path("/a\\b".to_owned()))
        );
    }
}

/// The watch of pane `side` tells that `names` came, as files.
fn files_came(state: &mut AppState, side: usize, names: &[&str]) {
    let generation = state.panes()[side].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation,
        changes: names
            .iter()
            .map(|name| rf_vfs::Change::Upsert(file(name)))
            .collect(),
    });
}

#[test]
fn what_a_paste_made_is_selected_as_it_shows() {
    // File Explorer selects the items a paste made. Here a copy of `z.txt` and `x.txt` into `a`,
    // which has an `x.txt` of its own: the job made `z.txt` and `x (2).txt`.
    let mut state = two_panes();
    let token = paste(&mut state, LEFT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(
            &[in_b("z.txt"), in_b("x.txt")],
            Some(DropEffect::Copy),
            12,
        ))),
    );
    let (spec, _) = the_submit(&effects);
    // The watch told of one of them before the job ended: it is selected, with the cursor.
    files_came(&mut state, LEFT, &["z.txt"]);
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.outputs = vec![in_a("z.txt"), in_a("x (2).txt")];
    });
    assert_eq!(rows(&state, LEFT), ["docs", "x.txt", "y.txt", "z.txt"]);
    assert!(
        effects.contains(&Effect::ScrollTo { side: LEFT, row: 3 }),
        "{effects:?}"
    );
    assert_eq!(selected(&state, LEFT), [3]);
    assert_eq!(cursor(&state, LEFT), Some(3));
    // The other joins the selection when it shows; the cursor stays on its item.
    files_came(&mut state, LEFT, &["x (2).txt"]);
    assert_eq!(
        rows(&state, LEFT),
        ["docs", "x (2).txt", "x.txt", "y.txt", "z.txt"]
    );
    assert_eq!(selected(&state, LEFT), [1, 4]);
    assert_eq!(cursor(&state, LEFT), Some(4));

    // A move pasted in the other pane, none of whose items shows yet: the first to show is
    // selected with the cursor, the next joins it.
    let token = put(&mut state, 2, true);
    state.clipboard_put(token, Ok(20));
    let token = paste(&mut state, RIGHT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(
            &[in_a("y.txt"), in_a("x.txt")],
            Some(DropEffect::Move),
            20,
        ))),
    );
    let (spec, _) = the_submit(&effects);
    let effects = finish(&mut state, 2, &spec, JobWork::Move, |report| {
        report.outputs = vec![in_b("y.txt"), in_b("x.txt")];
    });
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::ScrollTo { .. })),
        "{effects:?}"
    );
    files_came(&mut state, RIGHT, &["y.txt"]);
    assert_eq!(rows(&state, RIGHT), ["y.txt", "z.txt"]);
    assert_eq!(selected(&state, RIGHT), [0]);
    assert_eq!(cursor(&state, RIGHT), Some(0));
    files_came(&mut state, RIGHT, &["x.txt"]);
    assert_eq!(rows(&state, RIGHT), ["x.txt", "y.txt", "z.txt"]);
    assert_eq!(selected(&state, RIGHT), [0, 1]);
    assert_eq!(cursor(&state, RIGHT), Some(1));
}

#[test]
fn a_paste_selects_nothing_the_user_did_not_leave_to_it() {
    // The user picks a row while the paste runs: what the user chose stays, so that a key
    // pressed right then (Delete) acts on it.
    let mut state = two_panes();
    let token = paste(&mut state, LEFT);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_b("z.txt")], Some(DropEffect::Copy), 30))),
    );
    let (spec, _) = the_submit(&effects);
    state.track(Started {
        id: JobId(1),
        spec: Some(Arc::new(spec.clone())),
        work: JobWork::Copy,
        origin: Some(LEFT),
        via_history: false,
    });
    state.click_row(LEFT, 2, false, false);
    let mut report = JobReport::new(JobId(1), &spec);
    report.outputs = vec![in_a("z.txt")];
    state.job_event(JobEvent::Finished {
        id: JobId(1),
        report: Arc::new(report),
    });
    files_came(&mut state, LEFT, &["z.txt"]);
    assert_eq!(rows(&state, LEFT), ["docs", "x.txt", "y.txt", "z.txt"]);
    assert_eq!(selected(&state, LEFT), [2]);

    // The user picks a row while the clipboard is read, before the paste's job is submitted:
    // what the user chose stays too.
    let mut state = two_panes();
    let token = paste(&mut state, LEFT);
    state.click_row(LEFT, 1, false, false);
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_b("z.txt")], Some(DropEffect::Copy), 32))),
    );
    let (spec, _) = the_submit(&effects);
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.outputs = vec![in_a("z.txt")];
    });
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::ScrollTo { .. })),
        "{effects:?}"
    );
    files_came(&mut state, LEFT, &["z.txt"]);
    assert_eq!(selected(&state, LEFT), [1]);

    // Pasted into a folder row: the pane does not show where the items went, and its
    // selection stays.
    let mut state = two_panes();
    state.click_row(LEFT, 0, false, false);
    let effects = state.paste_from_menu(false);
    let ClipboardWork::ReadFiles { token } = the_work(&effects) else {
        panic!("expected a read, got {effects:?}");
    };
    let effects = state.clipboard_read(
        token,
        Ok(Some(files(&[in_b("z.txt")], Some(DropEffect::Copy), 31))),
    );
    let docs = in_a("docs");
    let into_docs = docs.join("z.txt").expect("a plain name");
    let (spec, _) = the_submit(&effects);
    assert_eq!(
        spec,
        transfer(TransferMode::Copy, &[(in_b("z.txt"), into_docs.clone())])
    );
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.outputs = vec![into_docs];
    });
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::ScrollTo { .. })),
        "{effects:?}"
    );
    assert_eq!(selected(&state, LEFT), [0]);
}

#[test]
fn a_cancelled_cut_still_on_the_clipboard_is_left_for_the_end_to_empty() {
    let busy = || Err(ClipboardFailure::Unavailable);
    let mut state = two_panes();
    assert_eq!(state.cut_left_on_clipboard(), None);
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(70));
    // A cut alone is no cancelled one.
    assert_eq!(state.cut_left_on_clipboard(), None);
    esc(&mut state);
    assert_eq!(
        esc(&mut state),
        Some(vec![Effect::Clipboard(ClipboardWork::ClearIf(70))])
    );
    // Its emptying is on its way, then failed once (another app held the clipboard): the app's
    // end is to try once more if it comes now, within the next look at the clipboard.
    assert_eq!(state.cut_left_on_clipboard(), Some(left_at(70)));
    assert_eq!(state.clipboard_cleared(70, busy()), []);
    assert_eq!(state.cut_left_on_clipboard(), Some(left_at(70)));
    // The end asks the state no more once it has begun.
    assert_eq!(state.begin_quit(), [Effect::Quit]);
    assert_eq!(state.cut_left_on_clipboard(), Some(left_at(70)));

    // Emptied: nothing left.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(71));
    esc(&mut state);
    esc(&mut state);
    assert_eq!(state.clipboard_cleared(71, Ok(true)), []);
    assert_eq!(state.cut_left_on_clipboard(), None);

    // Failed every time: the user has been told, and the end does not try again.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    state.clipboard_put(token, Ok(72));
    esc(&mut state);
    esc(&mut state);
    for _ in 1..CLEAR_TRIES {
        assert_eq!(state.clipboard_cleared(72, busy()), []);
    }
    assert_eq!(
        state.clipboard_cleared(72, busy()),
        [Effect::Notice(words::clipboard_not_emptied().to_owned())]
    );
    assert_eq!(state.cut_left_on_clipboard(), None);

    // Cancelled before its put told its count: the worker, which knows it, is to empty it.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    esc(&mut state);
    esc(&mut state);
    assert_eq!(
        state.cut_left_on_clipboard(),
        Some(CutLeft {
            put: Some(token),
            change: None,
        })
    );
    // Once the put has told it, the window empties it itself, as above.
    assert_eq!(
        state.clipboard_put(token, Ok(73)),
        [Effect::Clipboard(ClipboardWork::ClearIf(73))]
    );
    assert_eq!(state.cut_left_on_clipboard(), Some(left_at(73)));
    // A put that failed left nothing to empty.
    let mut state = two_panes();
    let token = put(&mut state, 1, true);
    esc(&mut state);
    esc(&mut state);
    assert_eq!(
        state.clipboard_put(token, Err(ClipboardFailure::Unavailable)),
        []
    );
    assert_eq!(state.cut_left_on_clipboard(), None);
}

/// A cut cancelled with Esc that the clipboard holds with the change count `change`.
fn left_at(change: u64) -> CutLeft {
    CutLeft {
        put: None,
        change: Some(change),
    }
}
