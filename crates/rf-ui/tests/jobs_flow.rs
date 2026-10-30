//! The window's file operations against a real job engine on temporary folders: the state asks,
//! the engine does it, and the files show what happened. The engine's journal is in a temporary
//! folder too, never in the user's data folder.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use rf_core::{Entry, EntryKind, Os, VPath};
use rf_jobs::{
    Conflict, ConflictAction, ConflictAnswer, DeleteMethod, DeleteSpec, EngineConfig, ErrorPolicy,
    JobEngine, JobEvent, JobId, JobSpec, Outcome, Resolver, TransferMode, TransferOptions,
    TransferSpec,
};
use rf_ui::dialogs::{ConfirmPurpose, ConflictChoice, Modal};
use rf_ui::jobs::{JobEffect, Jobs};
use rf_ui::loader::{LoadEvent, LoadTicket, Loader};
use rf_ui::pane::LoadStatus;
use rf_ui::resolver::{Asked, Question, UiResolver};
use rf_ui::state::{AppState, Effect, LEFT, Paging, RIGHT};
use rf_vfs::Vfs;
use slint::platform::Key;
use tempfile::TempDir;

/// The longest any test waits for the engine or the loader.
const WAIT: Duration = Duration::from_secs(20);

fn local(path: &Path) -> VPath {
    VPath::local(path).expect("a temporary folder is a local path")
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("a test file");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("the file is there")
}

/// A copy of `source` into the folder `dest` that asks about conflicts.
fn copy(source: &Path, dest: &Path) -> JobSpec {
    JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &[local(source)],
            &local(dest),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    )
}

/// Waits for the `Finished` event of `id` among `events` and gives its outcome.
fn outcome_of(events: &Receiver<JobEvent>, id: JobId) -> Outcome {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("the job ends in time");
        match events.recv_timeout(left) {
            Ok(JobEvent::Finished { id: ended, report }) if ended == id => {
                return report.outcome;
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("the engine is gone"),
        }
    }
}

/// An engine without a journal whose questions go to `resolver`.
fn engine(resolver: Arc<UiResolver>) -> (JobEngine, Receiver<JobEvent>) {
    JobEngine::start(EngineConfig::new(
        Vfs::with_local(),
        resolver as Arc<dyn Resolver>,
    ))
}

#[test]
fn conflict_question_round_trips() {
    let dir = TempDir::new().expect("a temporary folder");
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    std::fs::create_dir_all(&src).expect("src");
    std::fs::create_dir_all(&dst).expect("dst");
    write(&src.join("a.txt"), "new");
    write(&dst.join("a.txt"), "old");

    // The window that answers at once, keeping both: the question goes there through `post`
    // and the answer comes back through the resolver.
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let resolver = Arc::new_cyclic(|me: &Weak<UiResolver>| {
        let me = me.clone();
        UiResolver::new(move |question: Question| {
            let id = question.id;
            seen.lock().expect("the list").push(question);
            me.upgrade().is_some_and(|resolver| {
                resolver.answer_conflict(id, ConflictAnswer::once(ConflictAction::Rename))
            })
        })
    });
    let (engine, events) = engine(Arc::clone(&resolver));
    let id = engine.submit(copy(&src.join("a.txt"), &dst));
    assert_eq!(outcome_of(&events, id), Outcome::Completed);

    let asked = asked.lock().expect("the list");
    let [question] = asked.as_slice() else {
        panic!("one question: {asked:?}");
    };
    assert_eq!(question.job, id);
    let Asked::Conflict(conflict) = &question.asked else {
        panic!("a conflict: {question:?}");
    };
    assert_eq!(conflict.target, local(&dst.join("a.txt")));
    assert_eq!(conflict.incoming.kind, EntryKind::File);
    // Both are kept: the one that was there as it was, the new one under a numbered name.
    assert_eq!(read(&dst.join("a.txt")), "old");
    let numbered = if cfg!(target_os = "macos") {
        "a 2.txt"
    } else {
        "a (2).txt"
    };
    assert_eq!(read(&dst.join(numbered)), "new");
    assert_eq!(resolver.open_questions(), 0);
    assert!(engine.shutdown(WAIT));
}

#[test]
fn release_answers_cancel() {
    let dir = TempDir::new().expect("a temporary folder");
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    std::fs::create_dir_all(&src).expect("src");
    std::fs::create_dir_all(&dst).expect("dst");
    write(&src.join("a.txt"), "new");
    write(&dst.join("a.txt"), "old");

    // A window that never answers.
    let (asked, questions) = mpsc::channel();
    let asked = Mutex::new(asked);
    let resolver = Arc::new(UiResolver::new(move |question| {
        asked.lock().expect("the sender").send(question).is_ok()
    }));
    let (engine, events) = engine(Arc::clone(&resolver));
    let id = engine.submit(copy(&src.join("a.txt"), &dst));
    let question = questions.recv_timeout(WAIT).expect("the job asks");
    assert_eq!(question.job, id);
    assert_eq!(resolver.open_questions(), 1);

    resolver.release_all();
    assert!(resolver.is_closing());
    assert_eq!(outcome_of(&events, id), Outcome::Cancelled);
    // Nothing was written over or beside the file that was there.
    assert_eq!(read(&dst.join("a.txt")), "old");
    assert_eq!(std::fs::read_dir(&dst).expect("dst").count(), 1);

    // A question after the release is cancelled at once, without reaching the window.
    let conflict = Conflict {
        source: local(&src.join("a.txt")),
        target: local(&dst.join("a.txt")),
        incoming: Entry::new("a.txt", EntryKind::File),
        existing: Entry::new("a.txt", EntryKind::File),
    };
    assert_eq!(
        resolver.conflict(JobId(1), &conflict),
        ConflictAnswer::once(ConflictAction::Cancel)
    );
    assert!(questions.try_recv().is_err());
    assert!(engine.shutdown(WAIT));
}

/// The engine of the app on the journal folder `journal`, with its events and questions in
/// channels, as the window gets them.
fn jobs(journal: &Path) -> (Jobs, Receiver<JobEvent>, Receiver<Question>) {
    let (event_tx, events) = mpsc::channel();
    let (question_tx, questions) = mpsc::channel();
    let question_tx = Mutex::new(question_tx);
    let jobs = Jobs::start(
        journal,
        move |event| {
            let _ = event_tx.send(event);
        },
        move |question| {
            question_tx
                .lock()
                .expect("the sender")
                .send(question)
                .is_ok()
        },
    )
    .expect("the engine starts");
    (jobs, events, questions)
}

#[test]
fn quit_with_a_question_open_leaves_the_job_for_recovery() {
    let dir = TempDir::new().expect("a temporary folder");
    let journal = dir.path().join("journal");
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    std::fs::create_dir_all(&src).expect("src");
    std::fs::create_dir_all(&dst).expect("dst");
    write(&src.join("a.txt"), "new");
    write(&dst.join("a.txt"), "old");

    let (app, events, questions) = jobs(&journal);
    let started = app
        .carry_out(JobEffect::Submit {
            spec: copy(&src.join("a.txt"), &dst),
            origin: None,
        })
        .expect("a job");
    let question = questions.recv_timeout(WAIT).expect("the job asks");
    assert_eq!(question.job, started.id);

    // The window closes with the question open: the engine shuts down first, then the question
    // is released. The job waits for its answer and cannot stop before, so the shutdown waits
    // a little only, not its whole time.
    let closing = Instant::now();
    assert!(!app.close_down(rf_ui::jobs::QUIT_TIMEOUT));
    assert!(
        closing.elapsed() < rf_ui::jobs::QUIT_TIMEOUT,
        "{:?}",
        closing.elapsed()
    );
    assert!(app.is_closed());
    assert_eq!(outcome_of(&events, started.id), Outcome::Cancelled);
    assert_eq!(read(&dst.join("a.txt")), "old");
    drop(app);

    // The next start offers it again: it was cut off, not cancelled by the user.
    let (next, events, _questions) = jobs(&journal);
    let deadline = Instant::now() + WAIT;
    let recovered = loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("the journal is read in time");
        if let Ok(JobEvent::Recovered { jobs, .. }) = events.recv_timeout(left) {
            break jobs;
        }
    };
    assert_eq!(
        recovered.iter().map(|job| job.id).collect::<Vec<_>>(),
        [started.id]
    );
    assert!(next.close_down(WAIT));
}

/// The window's state with a real engine and loader under it.
struct Harness {
    jobs: Jobs,
    events: Receiver<JobEvent>,
    questions: Receiver<Question>,
    loader: Loader,
    loads: (Sender<LoadEvent>, Receiver<LoadEvent>),
    tickets: Vec<LoadTicket>,
    /// Whether the jobs' questions go to the state; while not, they wait in their channel.
    feed_questions: bool,
    journal: TempDir,
}

impl Harness {
    fn new() -> Harness {
        let journal = TempDir::new().expect("a journal folder");
        let (jobs, events, questions) = jobs(journal.path());
        Harness {
            jobs,
            events,
            questions,
            loader: Loader::new(Vfs::with_local()),
            loads: mpsc::channel(),
            tickets: Vec::new(),
            feed_questions: true,
            journal,
        }
    }

    /// The state with the left pane at `left` and the right one at `right`, both listed.
    fn open(&mut self, left: &Path, right: &Path) -> AppState {
        let (mut state, effects) = AppState::new(local(left), local(right), None);
        self.carry_out(&mut state, effects);
        self.until(&mut state, "both listings", |state| {
            state
                .panes()
                .iter()
                .all(|pane| pane.active_tab().status() == LoadStatus::Ready)
        });
        state
    }

    /// Does the job effects and the loads of `effects` as the controller would, and gives the
    /// others.
    fn carry_out(&mut self, state: &mut AppState, effects: Vec<Effect>) -> Vec<Effect> {
        let mut rest = Vec::new();
        for effect in effects {
            match effect {
                Effect::Job(effect) => {
                    if let Some(started) = self.jobs.carry_out(effect) {
                        state.track(started);
                    }
                    state.set_history(self.jobs.history());
                }
                Effect::Load(request) => {
                    let tx = self.loads.0.clone();
                    let ticket = self.loader.request(request, move |event| {
                        let _ = tx.send(event);
                    });
                    self.tickets.push(ticket);
                }
                other => rest.push(other),
            }
        }
        rest
    }

    /// Hands the engine's events and questions and the loader's events to the state, doing
    /// what it asks, until `done` holds; gives every other effect it asked for meanwhile.
    fn until(
        &mut self,
        state: &mut AppState,
        what: &str,
        done: impl Fn(&AppState) -> bool,
    ) -> Vec<Effect> {
        let deadline = Instant::now() + WAIT;
        let mut seen = Vec::new();
        while !done(state) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            let effects = if let Ok(event) = self.events.try_recv() {
                if matches!(event, JobEvent::Finished { .. }) {
                    state.set_history(self.jobs.history());
                }
                state.job_event(event)
            } else if let Some(question) = self
                .feed_questions
                .then(|| self.questions.try_recv().ok())
                .flatten()
            {
                state.question(question)
            } else if let Ok(event) = self.loads.1.try_recv() {
                state.loaded(event)
            } else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            let rest = self.carry_out(state, effects);
            seen.extend(rest);
        }
        seen
    }

    /// Does `effects` and waits until no job is under way.
    fn run(&mut self, state: &mut AppState, effects: Vec<Effect>) -> Vec<Effect> {
        let mut seen = self.carry_out(state, effects);
        seen.extend(self.until(state, "the jobs to end", |state| state.active_jobs() == 0));
        seen
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.tickets.clear();
        self.jobs.close_down(WAIT);
    }
}

fn key(state: &mut AppState, key: Key, shift: bool) -> Vec<Effect> {
    let mods = rf_ui::keys::Mods {
        shift,
        ..rf_ui::keys::Mods::default()
    };
    let command = rf_ui::keys::command_for(&char::from(key).to_string(), mods, Os::Windows)
        .expect("a shortcut");
    state
        .key(command, mods, Paging::rows(10), Instant::now())
        .expect("the window takes it")
}

/// The toolbar's 새 폴더 (Ctrl+Shift+N).
fn new_folder(state: &mut AppState) -> Vec<Effect> {
    state
        .key(
            rf_ui::keys::Command::NewFolder,
            rf_ui::keys::Mods::NONE,
            Paging::rows(10),
            Instant::now(),
        )
        .expect("the window takes it")
}

/// Clicks the row called `name` in pane `side`.
fn click(state: &mut AppState, side: usize, name: &str) {
    let row = state.panes()[side]
        .active_tab()
        .visible()
        .iter()
        .position(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"));
    state.click_row(side, row, false, false);
}

fn selected_names(state: &AppState, side: usize) -> Vec<String> {
    let tab = state.panes()[side].active_tab();
    tab.selection()
        .indices(tab.visible())
        .into_iter()
        .map(|row| tab.visible()[row].name.to_string_lossy().into_owned())
        .collect()
}

/// Two folders, `left` and `right`, in a temporary folder.
fn folders() -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().expect("a temporary folder");
    let (left, right) = (dir.path().join("left"), dir.path().join("right"));
    std::fs::create_dir_all(&left).expect("left");
    std::fs::create_dir_all(&right).expect("right");
    (dir, left, right)
}

#[test]
fn create_folder_selects_output() {
    let (_dir, left, right) = folders();
    write(&left.join("x.txt"), "x");
    let mut harness = Harness::new();
    let mut state = harness.open(&left, &right);

    assert_eq!(new_folder(&mut state), []);
    let effects = state.name_confirmed("자료");
    harness.run(&mut state, effects);
    assert!(left.join("자료").is_dir());
    // The listing (or the watch) brings the new folder, and it is the selection.
    harness.until(&mut state, "the new folder to show", |state| {
        selected_names(state, LEFT) == ["자료"]
    });
    let tab = state.panes()[LEFT].active_tab();
    assert_eq!(tab.selection().cursor(), Some(0));

    // A second one with the same name gets a numbered name, which is selected in turn.
    new_folder(&mut state);
    let effects = state.name_confirmed("자료");
    harness.run(&mut state, effects);
    let numbered = if cfg!(target_os = "macos") {
        "자료 2"
    } else {
        "자료 (2)"
    };
    assert!(left.join(numbered).is_dir());
    harness.until(&mut state, "the numbered folder to show", |state| {
        selected_names(state, LEFT) == [numbered]
    });
}

#[test]
fn rename_onto_existing_name_reports_and_keeps_both() {
    let (_dir, left, right) = folders();
    write(&left.join("a.txt"), "A");
    write(&left.join("b.txt"), "B");
    let mut harness = Harness::new();
    let mut state = harness.open(&left, &right);

    click(&mut state, LEFT, "a.txt");
    assert_eq!(key(&mut state, Key::F2, false), []);
    let effects = state.name_confirmed("b.txt");
    let seen = harness.run(&mut state, effects);
    assert!(
        seen.contains(&Effect::Notice(
            "이름을 바꾸지 못했습니다: 같은 이름의 항목이 이미 있습니다".to_owned()
        )),
        "{seen:?}"
    );
    // Nothing was written over: both files are as they were.
    assert_eq!(read(&left.join("a.txt")), "A");
    assert_eq!(read(&left.join("b.txt")), "B");
    assert_eq!(std::fs::read_dir(&left).expect("left").count(), 2);
}

#[test]
fn trash_refused_asks_before_permanent_delete() {
    let (_dir, left, right) = folders();
    write(&left.join("a.txt"), "A");
    let mut harness = Harness::new();
    let mut state = harness.open(&left, &right);

    click(&mut state, LEFT, "a.txt");
    let effects = key(&mut state, Key::Delete, false);
    let [Effect::CheckTrash { folder, paths }] = effects.as_slice() else {
        panic!("the trash is looked for first: {effects:?}");
    };
    assert_eq!(*folder, local(&left));
    assert_eq!(*paths, [local(&left.join("a.txt"))]);
    // Linux has no trash; elsewhere this test plays the folder without one, so that no file
    // of a test goes to the user's trash.
    if cfg!(target_os = "linux") {
        assert!(!rf_platform::trash_supported(folder));
    }
    let effects = state.trash_checked(paths.clone(), false);
    assert_eq!(harness.carry_out(&mut state, effects), []);
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("the user is asked: {:?}", state.modal());
    };
    assert_eq!(
        confirm.message,
        "휴지통을 쓸 수 없는 위치입니다.\n항목 1개를 영구 삭제할까요? 되돌릴 수 없습니다."
    );
    assert_eq!(state.active_jobs(), 0);
    assert!(
        left.join("a.txt").exists(),
        "nothing is deleted before a yes"
    );
    // No keeps it.
    assert_eq!(state.dialog_cancelled(), []);
    assert!(left.join("a.txt").exists());

    // On Linux the engine's own trash refuses too, and the same question follows.
    if cfg!(target_os = "linux") {
        let trash = JobSpec::Delete(DeleteSpec {
            paths: vec![local(&left.join("a.txt"))],
            method: DeleteMethod::Trash,
            errors: ErrorPolicy::Continue,
        });
        let effect = Effect::Job(JobEffect::Submit {
            spec: trash,
            origin: Some(LEFT),
        });
        harness.run(&mut state, vec![effect]);
        let Some(Modal::Confirm(confirm)) = state.modal() else {
            panic!("the refused item is offered for a permanent delete");
        };
        assert_eq!(
            confirm.purpose,
            ConfirmPurpose::DeletePermanently(vec![local(&left.join("a.txt"))])
        );
        assert!(left.join("a.txt").exists());
        assert_eq!(state.dialog_cancelled(), []);
        assert!(left.join("a.txt").exists());
    }

    // Yes deletes it for good.
    state.trash_checked(paths.clone(), false);
    let effects = state.confirm_accepted();
    harness.run(&mut state, effects);
    assert!(!left.join("a.txt").exists());
}

#[test]
fn move_then_undo_restores() {
    let (_dir, left, right) = folders();
    write(&left.join("m.txt"), "M");
    let mut harness = Harness::new();
    let mut state = harness.open(&left, &right);

    click(&mut state, LEFT, "m.txt");
    let dest = local(&right);
    let effects = state.transfer(LEFT, TransferMode::Move, &dest, TransferOptions::default());
    let seen = harness.run(&mut state, effects);
    assert!(!left.join("m.txt").exists());
    assert_eq!(read(&right.join("m.txt")), "M");
    let Some(Effect::Toast {
        token,
        text,
        action,
    }) = seen
        .iter()
        .find(|effect| matches!(effect, Effect::Toast { .. }))
    else {
        panic!("a toast: {seen:?}");
    };
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 1개를 이동했습니다", "되돌리기")
    );
    assert!(state.can_undo());

    // Its button undoes the move.
    let effects = state.toast_clicked(*token);
    assert_eq!(effects, [Effect::Job(JobEffect::Undo)]);
    let seen = harness.run(&mut state, effects);
    assert_eq!(read(&left.join("m.txt")), "M");
    assert!(!right.join("m.txt").exists());
    assert!(
        seen.iter().any(|effect| matches!(
            effect,
            Effect::Toast { text, .. } if text == "이동을 되돌렸습니다"
        )),
        "{seen:?}"
    );
    assert!(!state.can_undo());
    assert!(state.can_redo());
}

#[test]
fn undo_disabled_while_running() {
    let (dir, left, right) = folders();
    write(&left.join("first.txt"), "1");
    write(&left.join("m.txt"), "M");
    let blocker = dir.path().join("blocker");
    std::fs::create_dir_all(&blocker).expect("blocker");
    write(&blocker.join("b.txt"), "new");
    write(&right.join("b.txt"), "old");
    let mut harness = Harness::new();
    let mut state = harness.open(&left, &right);

    // Two moves: after the second is undone, the first is still there to undo.
    let dest = local(&right);
    for name in ["first.txt", "m.txt"] {
        click(&mut state, LEFT, name);
        let effects = state.transfer(LEFT, TransferMode::Move, &dest, TransferOptions::default());
        harness.run(&mut state, effects);
        harness.until(&mut state, "the moved file to leave", |state| {
            state.panes()[LEFT]
                .active_tab()
                .visible()
                .iter()
                .all(|entry| entry.name != name)
        });
    }
    assert!(state.can_undo());

    // A copy that waits for the answer to its conflict holds the lane the undo runs in. Its
    // question is held back from the window for now.
    harness.feed_questions = false;
    let effect = Effect::Job(JobEffect::Submit {
        spec: copy(&blocker.join("b.txt"), &right),
        origin: Some(RIGHT),
    });
    harness.carry_out(&mut state, vec![effect]);
    let question = harness.questions.recv_timeout(WAIT).expect("the copy asks");

    // Undo the move: it waits behind the copy, and while it does, no other undo or redo starts.
    let ctrl = rf_ui::keys::Mods {
        ctrl: true,
        ..rf_ui::keys::Mods::default()
    };
    let undo = rf_ui::keys::Command::Undo;
    let effects = state.key(undo.clone(), ctrl, Paging::rows(10), Instant::now());
    assert_eq!(effects, Some(vec![Effect::Job(JobEffect::Undo)]));
    harness.carry_out(&mut state, effects.unwrap_or_default());
    // The first move is next on the list, but nothing starts while the undo is under way.
    assert!(harness.jobs.history().top.is_some());
    assert!(!state.can_undo());
    assert!(!state.can_redo());
    assert_eq!(state.active_jobs(), 2);
    assert_eq!(harness.jobs.engine().undo(), None, "the engine agrees");
    assert_eq!(
        state.key(undo, ctrl, Paging::rows(10), Instant::now()),
        Some(vec![])
    );
    assert_eq!(
        state.key(
            rf_ui::keys::Command::Redo,
            ctrl,
            Paging::rows(10),
            Instant::now()
        ),
        Some(vec![])
    );
    assert!(!left.join("m.txt").exists() && right.join("m.txt").exists());

    // The question reaches the window now, and the copy skips its conflict.
    state.question(question);
    harness.feed_questions = true;
    assert!(matches!(state.modal(), Some(Modal::Conflict(_))));
    // It ends, the undo runs and puts the file back.
    let effects = state.conflict_answered(ConflictChoice::Skip, false);
    harness.run(&mut state, effects);
    assert_eq!(read(&left.join("m.txt")), "M");
    assert_eq!(read(&right.join("first.txt")), "1");
    assert_eq!(read(&right.join("b.txt")), "old");
    assert!(state.can_undo());
    assert!(state.can_redo());
    assert_eq!(
        state.key(
            rf_ui::keys::Command::Redo,
            ctrl,
            Paging::rows(10),
            Instant::now()
        ),
        Some(vec![Effect::Job(JobEffect::Redo)])
    );
}

#[test]
fn a_question_during_shutdown_waits_for_the_release_and_the_job_is_recovered() {
    let (_dir, left, right) = folders();
    write(&left.join("a.txt"), "new");
    write(&right.join("a.txt"), "old");
    let mut harness = Harness::new();
    let mut state = harness.open(&left, &right);

    // A copy that meets a conflict; its question is on its way to the window.
    harness.feed_questions = false;
    click(&mut state, LEFT, "a.txt");
    let dest = local(&right);
    let effects = state.transfer(LEFT, TransferMode::Copy, &dest, TransferOptions::default());
    harness.carry_out(&mut state, effects);
    let question = harness.questions.recv_timeout(WAIT).expect("the copy asks");
    let job = question.job;

    // The user closes the window meanwhile: a job runs, so the window asks, and the user ends.
    assert_eq!(state.request_close(), []);
    assert_eq!(state.confirm_accepted(), [Effect::Quit]);
    // The question reaches the window during the shutdown: it does not show, and nothing
    // answers it here.
    assert_eq!(state.question(question), []);
    assert_eq!(state.modal(), None);
    assert_eq!(harness.jobs.resolver().open_questions(), 1);

    // What `Effect::Quit` does: the engine shuts down first (the job waits for its answer, so
    // the whole time), then the release answers the question.
    assert!(!harness.jobs.close_down(Duration::from_millis(300)));
    assert_eq!(harness.jobs.resolver().open_questions(), 0);
    assert_eq!(outcome_of(&harness.events, job), Outcome::Cancelled);
    assert_eq!(read(&right.join("a.txt")), "old");
    assert_eq!(std::fs::read_dir(&right).expect("right").count(), 1);

    // The next start offers the job again: it was cut off, not cancelled by the user.
    let (next, events, _questions) = jobs(harness.journal.path());
    let deadline = Instant::now() + WAIT;
    let recovered = loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("the journal is read in time");
        if let Ok(JobEvent::Recovered { jobs, .. }) = events.recv_timeout(left) {
            break jobs;
        }
    };
    assert_eq!(
        recovered.iter().map(|job| job.id).collect::<Vec<_>>(),
        [job]
    );
    assert!(next.close_down(WAIT));
}

/// Where there is no trash (Linux), the undo of a copy trashes nothing: the copies stay until the
/// user has them deleted for good. These tests run only there: elsewhere the undo would send the
/// copies to the user's own trash.
#[cfg(target_os = "linux")]
mod without_a_trash {
    use rf_jobs::{Compared, CopyFound, CreatedItem};
    use rf_vfs::CancelToken;

    use super::*;

    /// Ctrl+Z, as the window takes it.
    fn undo_key(state: &mut AppState) -> Vec<Effect> {
        let ctrl = rf_ui::keys::Mods {
            ctrl: true,
            ..rf_ui::keys::Mods::default()
        };
        state
            .key(
                rf_ui::keys::Command::Undo,
                ctrl,
                Paging::rows(10),
                Instant::now(),
            )
            .expect("the window takes it")
    }

    /// A check of copies a worker is to make: its token, each copy as the copy left it with its
    /// original, and the copy's time.
    type Check = (
        u64,
        Vec<(CreatedItem, VPath)>,
        Option<std::time::SystemTime>,
    );

    /// The one check of copies among `effects`.
    fn the_check(effects: &[Effect]) -> Check {
        let checks: Vec<Check> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::CheckCopies { token, copies, at } => Some((*token, copies.clone(), *at)),
                _ => None,
            })
            .collect();
        let [check] = checks.as_slice() else {
            panic!("one check of the copies: {effects:?}");
        };
        check.clone()
    }

    /// The copies of `check` with their originals, in path order.
    fn pairs((_, copies, _): &Check) -> Vec<(VPath, VPath)> {
        let mut pairs: Vec<(VPath, VPath)> = copies
            .iter()
            .map(|(made, original)| (made.path.clone(), original.clone()))
            .collect();
        pairs.sort_by_key(|(copy, _)| copy.to_string());
        pairs
    }

    /// What the worker finds for `check`, as the controller has it look.
    fn look(harness: &Harness, (_, copies, at): &Check) -> Vec<CopyFound> {
        copies
            .iter()
            .map(|(made, original)| {
                harness
                    .loader
                    .check_copy(made, *at, original, &CancelToken::new())
            })
            .collect()
    }

    /// The user is offered the copies for a permanent delete once their originals are found as
    /// they were, and the copy is undone once they are gone.
    #[test]
    fn undo_of_a_copy_without_a_trash_offers_the_copies_for_good() {
        let (_dir, left, right) = folders();
        write(&left.join("a.txt"), "A");
        std::fs::create_dir_all(left.join("docs")).expect("docs");
        write(&left.join("docs").join("x.txt"), "X");
        write(&left.join("older.txt"), "O");
        let mut harness = Harness::new();
        let mut state = harness.open(&left, &right);

        // An older job, to undo once the copy is undone.
        click(&mut state, LEFT, "older.txt");
        key(&mut state, Key::F2, false);
        let effects = state.name_confirmed("renamed.txt");
        harness.run(&mut state, effects);
        let older = harness.jobs.history().top.expect("the rename's record");

        click(&mut state, LEFT, "a.txt");
        let docs = state.panes()[LEFT]
            .active_tab()
            .visible()
            .iter()
            .position(|entry| entry.name == "docs")
            .expect("docs is listed");
        state.click_row(LEFT, docs, true, false);
        let dest = local(&right);
        let effects = state.transfer(LEFT, TransferMode::Copy, &dest, TransferOptions::default());
        harness.run(&mut state, effects);
        assert_eq!(read(&right.join("docs").join("x.txt")), "X");
        let copied = harness.jobs.history().top.expect("the copy's record");

        // The undo trashes nothing, keeps its record for another try, and asks a worker about the
        // copies and their originals before anything is offered.
        let effects = undo_key(&mut state);
        let seen = harness.run(&mut state, effects);
        assert!(right.join("a.txt").exists() && right.join("docs").exists());
        assert_eq!(harness.jobs.history().top.as_ref(), Some(&copied));
        assert_eq!(state.modal(), None);
        let check = the_check(&seen);
        assert_eq!(
            pairs(&check),
            [
                (local(&right.join("a.txt")), local(&left.join("a.txt"))),
                (local(&right.join("docs")), local(&left.join("docs"))),
            ]
        );
        let found = look(&harness, &check);
        assert_eq!(found, [CopyFound::SAME, CopyFound::SAME]);

        // The copies are offered, each by its whole path, as a danger confirmation.
        state.copies_checked(check.0, &found);
        let Some(Modal::Confirm(confirm)) = state.modal() else {
            panic!("the copies are offered: {:?}", state.modal());
        };
        assert!(confirm.danger);
        let mut lines = confirm.lines.clone();
        lines.sort();
        assert_eq!(
            lines,
            [
                local(&right.join("a.txt")).to_string(),
                local(&right.join("docs")).to_string()
            ]
        );

        // Yes: the delete of the copies looks at each one and its original once more, then
        // the copies go for good, the originals stay, and the copy is undone: the next undo is
        // the older job's.
        let effects = state.confirm_accepted();
        assert!(
            right.join("a.txt").exists(),
            "nothing is deleted before the job runs"
        );
        let seen = harness.run(&mut state, effects);
        assert!(!right.join("a.txt").exists() && !right.join("docs").exists());
        assert_eq!(read(&left.join("a.txt")), "A");
        assert_eq!(read(&left.join("docs").join("x.txt")), "X");
        assert!(
            seen.iter().any(|effect| matches!(
                effect,
                Effect::Toast { text, .. } if text == "복사본 2개를 영구 삭제해 복사를 되돌렸습니다"
            )),
            "{seen:?}"
        );
        assert_eq!(harness.jobs.history().top, Some(older));
    }

    /// An original removed after the copy: the copy is the only one left, and is never offered.
    #[test]
    fn undo_of_a_copy_whose_original_is_gone_keeps_the_copy() {
        let (_dir, left, right) = folders();
        write(&left.join("a.txt"), "A");
        let mut harness = Harness::new();
        let mut state = harness.open(&left, &right);
        click(&mut state, LEFT, "a.txt");
        let dest = local(&right);
        let effects = state.transfer(LEFT, TransferMode::Copy, &dest, TransferOptions::default());
        harness.run(&mut state, effects);
        let copied = harness.jobs.history().top.expect("the copy's record");
        std::fs::remove_file(left.join("a.txt")).expect("the original goes");

        let effects = undo_key(&mut state);
        let seen = harness.run(&mut state, effects);
        let check = the_check(&seen);
        let found = look(&harness, &check);
        assert_eq!(
            found,
            [CopyFound {
                copy: Compared::Same,
                original: Compared::Missing
            }]
        );
        let effects = state.copies_checked(check.0, &found);
        assert_eq!(state.modal(), None, "nothing is offered");
        let Some((token, text)) = effects.iter().find_map(|effect| match effect {
            Effect::Toast { token, text, .. } => Some((*token, text.clone())),
            _ => None,
        }) else {
            panic!("a toast: {effects:?}");
        };
        assert_eq!(text, "항목 1개를 처리하지 못했습니다");
        state.toast_clicked(token);
        let Some(Modal::Failures(failures)) = state.modal() else {
            panic!("the copy and why: {:?}", state.modal());
        };
        assert_eq!(
            failures.lines,
            [format!(
                "{}: 휴지통으로 보낼 수 없습니다. 원본이 없어 복사본을 지우지 않았습니다",
                local(&right.join("a.txt"))
            )]
        );
        assert_eq!(read(&right.join("a.txt")), "A");
        // The record stays: the undo can be tried again.
        assert_eq!(harness.jobs.history().top, Some(copied));
    }

    /// The copies offered, and something changed before the user said yes: an original removed,
    /// or a copy written to. The delete of the copies keeps that copy.
    #[test]
    fn a_change_after_the_offer_keeps_that_copy() {
        for change in ["original removed", "copy written"] {
            let (_dir, left, right) = folders();
            write(&left.join("a.txt"), "A");
            write(&left.join("b.txt"), "B");
            let mut harness = Harness::new();
            let mut state = harness.open(&left, &right);
            click(&mut state, LEFT, "a.txt");
            state.click_row(LEFT, 1, true, false);
            let dest = local(&right);
            let effects =
                state.transfer(LEFT, TransferMode::Copy, &dest, TransferOptions::default());
            harness.run(&mut state, effects);
            let copied = harness.jobs.history().top.expect("the copy's record");
            let effects = undo_key(&mut state);
            let seen = harness.run(&mut state, effects);
            let check = the_check(&seen);
            let found = look(&harness, &check);
            state.copies_checked(check.0, &found);
            assert!(
                matches!(state.modal(), Some(Modal::Confirm(_))),
                "{change}: the copies are offered"
            );

            if change == "original removed" {
                std::fs::remove_file(left.join("b.txt")).expect("the original goes");
            } else {
                write(&right.join("b.txt"), "written since");
            }
            let effects = state.confirm_accepted();
            let seen = harness.run(&mut state, effects);
            assert!(
                seen.iter().any(|effect| matches!(
                    effect,
                    Effect::Toast { text, .. } if text == "항목 1개를 처리하지 못했습니다"
                )),
                "{change}: {seen:?}"
            );
            assert!(!right.join("a.txt").exists(), "{change}");
            assert!(right.join("b.txt").exists(), "{change}");
            // The record stays: the undo can be tried again.
            assert_eq!(harness.jobs.history().top, Some(copied), "{change}");
        }
    }
}
