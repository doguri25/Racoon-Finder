//! The copies an undo of a copy could not send to the trash: offered for a permanent delete
//! only once their originals are found as they were.

use rf_core::Os;
use rf_jobs::{
    CreatedItem, DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobEvent,
    JobSpec, Outcome, TransferItem, TransferMode, TransferOptions, TransferSpec, UndoRecord,
    numbered_name,
};

use super::*;
use crate::dialogs::{ConfirmPurpose, Modal};
use crate::jobs::{History, JobEffect, Started};
use crate::loader::{CopyFound, Original};
use crate::state::copy_origins;
use crate::words::JobWork;

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn in_b(name: &str) -> VPath {
    folder("b").join(name).expect("a plain name")
}

fn made(path: VPath, is_dir: bool) -> CreatedItem {
    CreatedItem {
        path,
        is_dir,
        size: 1,
        modified: None,
        identity: None,
    }
}

/// The copy of `names` from `a` into `b`, and the record of what it made there.
fn copy_of(names: &[&str]) -> (JobSpec, UndoRecord) {
    let sources: Vec<VPath> = names.iter().map(|name| in_a(name)).collect();
    let spec = TransferSpec::into_dir(
        TransferMode::Copy,
        &sources,
        &folder("b"),
        TransferOptions::default(),
    )
    .expect("a transfer");
    let record = UndoRecord::Copied {
        created: names.iter().map(|name| made(in_b(name), false)).collect(),
        overwritten: 0,
        at: None,
    };
    (JobSpec::Transfer(spec), record)
}

/// Ends the job `id` of `spec`, followed as `work`, with the report `edit` makes.
fn finish(
    state: &mut AppState,
    id: u64,
    spec: &JobSpec,
    work: JobWork,
    edit: impl FnOnce(&mut JobReport),
) -> Vec<Effect> {
    state.track(Started {
        id: JobId(id),
        spec: Some(Arc::new(spec.clone())),
        work,
        origin: Some(LEFT),
        via_history: matches!(work, JobWork::Undo | JobWork::Redo),
    });
    let mut report = JobReport::new(JobId(id), spec);
    edit(&mut report);
    without_space_reads(state.job_event(JobEvent::Finished {
        id: JobId(id),
        report: Arc::new(report),
    }))
}

fn refused(path: VPath) -> ItemFailure {
    ItemFailure {
        source: Some(path),
        target: None,
        stage: rf_jobs::Stage::Delete,
        kind: ErrorKind::Unsupported,
        message: String::new(),
        reason: Some(FailureReason::TrashRefused),
    }
}

/// The undo of `record`, whose trash refused each of `copies`.
fn undo_refused(
    state: &mut AppState,
    id: u64,
    record: &UndoRecord,
    copies: &[VPath],
) -> Vec<Effect> {
    let spec = JobSpec::Undo(record.clone());
    finish(state, id, &spec, JobWork::Undo, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures = copies.iter().cloned().map(refused).collect();
    })
}

/// The one check of copies among `effects`, which come with the notice that it runs: its
/// token, each copy with its original, and the notice's token.
fn the_check(effects: &[Effect]) -> (u64, Vec<(VPath, VPath)>, u64) {
    match effects {
        [
            Effect::Busy { token: note, text },
            Effect::CheckCopies { token, copies, .. },
        ] => {
            assert_eq!(text, "원본을 확인하는 중…");
            let pairs = copies
                .iter()
                .map(|(made, original)| (made.path.clone(), original.clone()))
                .collect();
            (*token, pairs, *note)
        }
        other => panic!("expected one check under its notice, got {other:?}"),
    }
}

/// What a worker finds of a copy whose original is as `original` says.
fn original(original: Original) -> CopyFound {
    CopyFound {
        copy: Original::Same,
        original,
    }
}

/// The token, text and button of the one toast among `effects`.
fn the_toast(effects: &[Effect]) -> (u64, String, String) {
    let toasts: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Toast {
                token,
                text,
                action,
            } => Some((*token, text.clone(), action.clone())),
            _ => None,
        })
        .collect();
    match toasts.as_slice() {
        [one] => one.clone(),
        other => panic!("expected one toast, got {other:?} in {effects:?}"),
    }
}

/// The lines of the list the toast `token` opens.
fn details(state: &mut AppState, token: u64) -> Vec<String> {
    assert_eq!(state.toast_clicked(token), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items: {:?}", state.modal());
    };
    let lines = failures.lines.clone();
    state.dialog_cancelled();
    lines
}

#[test]
fn copies_are_traced_to_their_sources() {
    let docs_into = |name: &str| in_b("docs").join(name).expect("a name");
    let numbered = |name: &str, is_dir| {
        numbered_name(std::ffi::OsStr::new(name), 2, is_dir, Os::CURRENT)
            .to_string_lossy()
            .into_owned()
    };
    let spec = TransferSpec {
        mode: TransferMode::Copy,
        items: vec![
            TransferItem {
                source: in_a("a.txt"),
                target: in_b("a.txt"),
            },
            // A folder merged into the one of its name that was there.
            TransferItem {
                source: in_a("docs"),
                target: in_b("docs"),
            },
            TransferItem {
                source: in_a("photos"),
                target: in_b("photos"),
            },
            // Two items of one name from two folders: either could be any copy of the name.
            TransferItem {
                source: folder("c").join("x.txt").expect("a name"),
                target: in_b("x.txt"),
            },
            TransferItem {
                source: folder("d").join("x.txt").expect("a name"),
                target: in_b("x.txt"),
            },
        ],
        options: TransferOptions::default(),
    };
    let created = [
        made(in_b("a.txt"), false),
        made(docs_into("new.txt"), false),
        made(docs_into("sub").join("deep.txt").expect("a name"), false),
        // A target that was taken, kept under a numbered name.
        made(in_b(&numbered("photos", true)), true),
        made(in_b("x.txt"), false),
        made(in_b(&numbered("x.txt", false)), false),
        made(in_b("other.txt"), false),
    ];
    let origins = copy_origins(&spec, &created);
    let sources: Vec<Option<VPath>> = origins.iter().map(|(_, from)| from.clone()).collect();
    assert_eq!(
        sources,
        [
            Some(in_a("a.txt")),
            Some(in_a("docs").join("new.txt").expect("a name")),
            Some(
                in_a("docs")
                    .join("sub")
                    .and_then(|sub| sub.join("deep.txt"))
                    .expect("a name")
            ),
            Some(in_a("photos")),
            None,
            None,
            None,
        ]
    );
    assert_eq!(origins[0].0, in_b("a.txt"));
}

#[test]
fn an_undo_the_trash_refused_offers_the_copies_once_their_originals_are_found() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let (spec, record) = copy_of(&["a.txt", "b.txt"]);
    finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.undo = Some(record.clone());
    });
    state.set_history(History {
        top: Some(record.clone()),
        can_redo: false,
    });
    let copies = [in_b("a.txt"), in_b("b.txt")];
    let effects = undo_refused(&mut state, 2, &record, &copies);
    // Nothing is asked, nor told but that a worker looks at the originals, before it has.
    let (token, pairs, note) = the_check(&effects);
    assert_eq!(
        pairs,
        [
            (in_b("a.txt"), in_a("a.txt")),
            (in_b("b.txt"), in_a("b.txt"))
        ]
    );
    // The worker checks each copy against what the copy left, at the copy's time.
    let [
        _,
        Effect::CheckCopies {
            copies: made, at, ..
        },
    ] = effects.as_slice()
    else {
        panic!("the check");
    };
    let UndoRecord::Copied {
        created,
        at: copied_at,
        ..
    } = &record
    else {
        panic!("a copy's record");
    };
    assert_eq!(
        made.iter()
            .map(|(item, _)| item.clone())
            .collect::<Vec<_>>(),
        *created
    );
    assert_eq!(at, copied_at);
    assert_eq!(state.modal(), None);

    // The notice goes once the worker has answered.
    assert_eq!(
        state.copies_checked(token, &[CopyFound::SAME, CopyFound::SAME]),
        [Effect::NotBusy(note)]
    );
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("the copies are offered: {:?}", state.modal());
    };
    assert!(confirm.danger, "Enter does not confirm it");
    assert_eq!(
        confirm.message,
        "복사본을 휴지통으로 보낼 수 없습니다. 원본은 그대로 있습니다."
    );
    // Every copy by its whole path.
    assert_eq!(
        confirm.lines,
        [in_b("a.txt").to_string(), in_b("b.txt").to_string()]
    );
    assert_eq!(
        confirm.question,
        "복사본 2개를 영구 삭제할까요? 되돌릴 수 없습니다."
    );
    assert_eq!(confirm.ok, "영구 삭제");
    assert!(matches!(
        &confirm.purpose,
        ConfirmPurpose::DeleteCopies { copies: c, originals: o, record: r }
            if *c == copies && *o == [in_a("a.txt"), in_a("b.txt")] && **r == record
    ));
}

/// The state at the offer of the copies of `names` in `b`, after the undo of their copy from
/// `a` could not trash them and their originals were found as they were; and the copy's record.
fn offered(names: &[&str]) -> (AppState, UndoRecord) {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let (spec, record) = copy_of(names);
    finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.undo = Some(record.clone());
    });
    let copies: Vec<VPath> = names.iter().map(|name| in_b(name)).collect();
    let (token, _, _) = the_check(&undo_refused(&mut state, 2, &record, &copies));
    state.copies_checked(token, &vec![CopyFound::SAME; names.len()]);
    assert!(matches!(state.modal(), Some(Modal::Confirm(_))));
    (state, record)
}

#[test]
fn the_copies_go_for_good_once_found_as_they_were_again() {
    let (mut state, record) = offered(&["a.txt", "b.txt"]);
    let copies = [in_b("a.txt"), in_b("b.txt")];
    // Yes: a worker looks at the copies and their originals once more, and only with all of
    // them as they were are exactly the copies deleted, for good.
    let (token, pairs, note) = the_check(&state.confirm_accepted());
    assert_eq!(state.modal(), None);
    assert_eq!(
        pairs,
        [
            (in_b("a.txt"), in_a("a.txt")),
            (in_b("b.txt"), in_a("b.txt"))
        ]
    );
    let effects = state.copies_checked(token, &[CopyFound::SAME, CopyFound::SAME]);
    let delete = JobSpec::Delete(DeleteSpec {
        paths: copies.to_vec(),
        method: DeleteMethod::Permanent,
        errors: ErrorPolicy::Continue,
    });
    assert_eq!(
        effects,
        [
            Effect::NotBusy(note),
            Effect::Job(JobEffect::Submit {
                spec: delete.clone(),
                origin: None
            })
        ]
    );
    // Once they are all gone, the copy is undone.
    let effects = finish(&mut state, 3, &delete, JobWork::Delete, |_| {});
    assert!(
        effects.contains(&Effect::Job(JobEffect::ForgetUndo(record.clone()))),
        "{effects:?}"
    );
    assert_eq!(
        the_toast(&effects).1,
        "복사본 2개를 영구 삭제해 복사를 되돌렸습니다"
    );
}

#[test]
fn a_change_after_the_offer_deletes_nothing() {
    // The state at the offer of the copies of a, b and c, and the check the user's yes starts.
    let confirmed = || {
        let (mut state, _) = offered(&["a.txt", "b.txt", "c.txt"]);
        let (token, _, note) = the_check(&state.confirm_accepted());
        (state, token, note)
    };

    // An original removed meanwhile, or changed; a copy changed or removed meanwhile: nothing
    // is deleted, and the toast's list says why for each.
    for (found, why) in [
        (
            original(Original::Missing),
            "원본이 없어 복사본을 지우지 않았습니다",
        ),
        (
            original(Original::Changed),
            "원본이 바뀌어 복사본을 지우지 않았습니다",
        ),
        (
            CopyFound {
                copy: Original::Changed,
                original: Original::Unknown,
            },
            "복사본이 바뀌어 지우지 않았습니다",
        ),
        (
            CopyFound {
                copy: Original::Missing,
                original: Original::Unknown,
            },
            "복사본이 이미 없습니다",
        ),
        (
            CopyFound {
                copy: Original::Unknown,
                original: Original::Unknown,
            },
            "복사본을 확인할 수 없어 지우지 않았습니다",
        ),
    ] {
        let (mut state, token, note) = confirmed();
        let effects = state.copies_checked(token, &[CopyFound::SAME, found, CopyFound::SAME]);
        assert_eq!(effects.first(), Some(&Effect::NotBusy(note)));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Job(_))),
            "nothing is deleted: {effects:?}"
        );
        assert_eq!(state.modal(), None);
        let (token, text, action) = the_toast(&effects);
        assert_eq!(
            (text.as_str(), action.as_str()),
            ("복사본 3개를 지우지 않았습니다", "자세히")
        );
        assert_eq!(
            details(&mut state, token),
            [
                format!("{}: 휴지통으로 보낼 수 없습니다", in_b("a.txt")),
                format!("{}: 휴지통으로 보낼 수 없습니다. {why}", in_b("b.txt")),
                format!("{}: 휴지통으로 보낼 수 없습니다", in_b("c.txt")),
            ]
        );
    }

    // The app ending meanwhile: nothing is deleted either.
    let (mut state, token, _) = confirmed();
    state.begin_quit();
    assert_eq!(state.copies_checked(token, &[CopyFound::SAME; 3]), []);
}

#[test]
fn no_offer_while_an_original_is_gone_or_changed() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let (spec, record) = copy_of(&["a.txt", "b.txt", "c.txt"]);
    finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.undo = Some(record.clone());
    });
    let copies = [in_b("a.txt"), in_b("b.txt"), in_b("c.txt")];
    let (token, _, note) = the_check(&undo_refused(&mut state, 2, &record, &copies));
    let effects = state.copies_checked(
        token,
        &[
            CopyFound::SAME,
            original(Original::Missing),
            original(Original::Changed),
        ],
    );
    assert_eq!(effects.first(), Some(&Effect::NotBusy(note)));
    assert_eq!(state.modal(), None, "nothing is offered");
    let (token, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 3개를 처리하지 못했습니다", "자세히")
    );
    assert_eq!(
        details(&mut state, token),
        [
            format!("{}: 휴지통으로 보낼 수 없습니다", in_b("a.txt")),
            format!(
                "{}: 휴지통으로 보낼 수 없습니다. 원본이 없어 복사본을 지우지 않았습니다",
                in_b("b.txt")
            ),
            format!(
                "{}: 휴지통으로 보낼 수 없습니다. 원본이 바뀌어 복사본을 지우지 않았습니다",
                in_b("c.txt")
            ),
        ]
    );
    // A second answer to the same check changes nothing.
    assert_eq!(state.copies_checked(token, &[CopyFound::SAME; 3]), []);
}

#[test]
fn a_check_that_was_replaced_is_dropped() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let (spec, record) = copy_of(&["a.txt"]);
    finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.undo = Some(record.clone());
    });
    let copies = [in_b("a.txt")];
    let (first, _, _) = the_check(&undo_refused(&mut state, 2, &record, &copies));
    let (second, _, _) = the_check(&undo_refused(&mut state, 3, &record, &copies));
    assert_ne!(first, second);
    assert_eq!(state.copies_checked(first, &[CopyFound::SAME]), []);
    assert_eq!(state.modal(), None);
    state.copies_checked(second, &[CopyFound::SAME]);
    assert!(matches!(state.modal(), Some(Modal::Confirm(_))));
}

#[test]
fn copies_of_unknown_origin_are_never_offered() {
    // A copy of an earlier run: this run does not know where its copies came from.
    let mut state = ready(vec![file("a.txt")], vec![]);
    let (_, record) = copy_of(&["a.txt"]);
    let effects = undo_refused(&mut state, 1, &record, &[in_b("a.txt")]);
    assert_eq!(state.modal(), None);
    let (token, text, _) = the_toast(&effects);
    assert_eq!(text, "항목 1개를 처리하지 못했습니다");
    assert_eq!(
        details(&mut state, token),
        [format!(
            "{}: 휴지통으로 보낼 수 없습니다. 원본을 확인할 수 없어 복사본을 지우지 않았습니다",
            in_b("a.txt")
        )]
    );
}

#[test]
fn only_the_undo_of_a_copy_offers_its_copies() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    // The redo of an undone trash sends the items to the trash again; a refusal there is a
    // failure like any other.
    let record = UndoRecord::Restored {
        items: vec![rf_jobs::TrashedItem {
            original: in_a("a.txt"),
            location: None,
        }],
    };
    let effects = undo_refused(&mut state, 1, &record, &[in_a("a.txt")]);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::CheckCopies { .. })),
        "{effects:?}"
    );
    assert_eq!(state.modal(), None);
    let (token, text, _) = the_toast(&effects);
    assert_eq!(text, "항목 1개를 처리하지 못했습니다");
    assert_eq!(
        details(&mut state, token),
        [format!("{}: 휴지통으로 보낼 수 없습니다", in_a("a.txt"))]
    );
}

#[test]
fn a_delete_of_the_copies_that_fails_keeps_the_record() {
    let (mut state, _) = offered(&["a.txt", "b.txt"]);
    let (token, _, _) = the_check(&state.confirm_accepted());
    let effects = state.copies_checked(token, &[CopyFound::SAME; 2]);
    let [_, Effect::Job(JobEffect::Submit { spec: delete, .. })] = effects.as_slice() else {
        panic!("the delete: {effects:?}");
    };
    let effects = finish(&mut state, 3, delete, JobWork::Delete, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(ItemFailure {
            reason: None,
            kind: ErrorKind::Busy,
            ..refused(in_b("b.txt"))
        });
    });
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Job(JobEffect::ForgetUndo(_)))),
        "{effects:?}"
    );
    assert_eq!(the_toast(&effects).1, "항목 1개를 처리하지 못했습니다");
}
