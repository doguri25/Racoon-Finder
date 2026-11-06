//! Making links and undoing them (OPS-08, OPS-12), with the symbolic links of a Unix disk. The
//! shortcut, alias and junction of the other systems are made by `rf-platform`, which a job
//! reaches the same way.
#![cfg(unix)]

mod common;

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use common::*;
use rf_core::{ErrorKind, Os};
use rf_jobs::testkit::wait_finished;
use rf_jobs::{
    DefaultLanes, EngineConfig, ExecEnv, FailureReason, JobEngine, JobEvent, JobId, JobKind,
    JobSpec, LaneId, LanePolicy, LinkKind, NonInteractive, Outcome, Stage, UndoRecord, execute,
};
use rf_vfs::Vfs;

const WAIT: Duration = Duration::from_secs(60);

fn link_job(links: &[(&std::path::Path, &std::path::Path, LinkKind)]) -> JobSpec {
    JobSpec::MakeLink {
        links: links
            .iter()
            .map(|(target, at, kind)| (v(target), v(at), *kind))
            .collect(),
    }
}

fn engine() -> (JobEngine, Receiver<JobEvent>) {
    JobEngine::start(EngineConfig::new(
        Vfs::with_local(),
        Arc::new(NonInteractive),
    ))
}

#[test]
fn a_symbolic_link_is_made_selected_and_recorded() {
    let f = fx();
    let target = f.src.join("a.txt");
    std::fs::write(&target, "A").unwrap();
    let at = f.dst.join("a.txt");
    let report = run(&link_job(&[(&target, &at, LinkKind::Symlink)]));
    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
    assert_eq!(report.kind, JobKind::Link);
    assert_eq!(std::fs::read_link(&at).unwrap(), target);
    assert_eq!(report.outputs, [v(&at)]);
    assert_eq!(
        report.undo,
        Some(UndoRecord::LinksMade {
            at: vec![(v(&at), LinkKind::Symlink, v(&target))]
        })
    );
    assert_eq!((report.planned.files, report.done.files), (1, 1));
    assert_eq!(report.warnings, Vec::new());
    // The target is as it was.
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "A");
}

#[test]
fn undoing_a_link_removes_only_the_link_it_made() {
    let f = fx();
    let folder = f.src.join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("inner.txt"), "I").unwrap();
    let file = f.src.join("a.txt");
    std::fs::write(&file, "A").unwrap();
    let (to_folder, to_file) = (f.dst.join("folder"), f.dst.join("a.txt"));
    let (e, rx) = engine();
    let id = e.submit(link_job(&[
        (&folder, &to_folder, LinkKind::Symlink),
        (&file, &to_file, LinkKind::Symlink),
    ]));
    assert_eq!(wait_finished(&rx, id, WAIT).outcome, Outcome::Completed);
    let id = e.undo().expect("something to undo");
    let undone = wait_finished(&rx, id, WAIT);
    assert_eq!(undone.outcome, Outcome::Completed, "{:?}", undone.failures);
    // Both links are gone; the folder, its content and the file are not.
    assert!(std::fs::symlink_metadata(&to_folder).is_err());
    assert!(std::fs::symlink_metadata(&to_file).is_err());
    assert_eq!(
        std::fs::read_to_string(folder.join("inner.txt")).unwrap(),
        "I"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    // A redo makes them again, under the same names.
    let id = e.redo().expect("something to redo");
    let redone = wait_finished(&rx, id, WAIT);
    assert_eq!(redone.outcome, Outcome::Completed, "{:?}", redone.failures);
    assert_eq!(std::fs::read_link(&to_folder).unwrap(), folder);
    assert_eq!(std::fs::read_link(&to_file).unwrap(), file);
    assert!(e.shutdown(WAIT));
}

#[test]
fn an_undo_refuses_a_file_that_took_the_links_place() {
    let f = fx();
    let target = f.src.join("a.txt");
    std::fs::write(&target, "A").unwrap();
    let at = f.dst.join("link");
    let made = run(&link_job(&[(&target, &at, LinkKind::Symlink)]));
    std::fs::remove_file(&at).unwrap();
    std::fs::write(&at, "mine").unwrap();
    let report = run(&JobSpec::Undo(made.undo.unwrap()));
    assert_eq!(report.outcome, Outcome::CompletedWithErrors);
    let failure = &report.failures[0];
    assert_eq!(
        (failure.stage, failure.reason.clone()),
        (Stage::Undo, Some(FailureReason::ChangedSinceLinked))
    );
    assert_eq!(failure.source, Some(v(&at)));
    // Nothing is removed, and nothing is recorded to do again.
    assert_eq!(std::fs::read_to_string(&at).unwrap(), "mine");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "A");
    assert_eq!(report.undo, None);
}

#[test]
fn an_undo_refuses_a_link_that_points_elsewhere() {
    let f = fx();
    let (target, other) = (f.src.join("a.txt"), f.src.join("b.txt"));
    std::fs::write(&target, "A").unwrap();
    std::fs::write(&other, "B").unwrap();
    let at = f.dst.join("link");
    let made = run(&link_job(&[(&target, &at, LinkKind::Symlink)]));
    std::fs::remove_file(&at).unwrap();
    std::os::unix::fs::symlink(&other, &at).unwrap();
    let report = run(&JobSpec::Undo(made.undo.unwrap()));
    assert_eq!(report.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        report.failures[0].reason,
        Some(FailureReason::ChangedSinceLinked)
    );
    assert_eq!(std::fs::read_link(&at).unwrap(), other);
    assert!(target.exists() && other.exists());
}

#[test]
fn an_undo_refuses_a_link_of_another_kind() {
    let f = fx();
    let target = f.src.join("a.txt");
    std::fs::write(&target, "A").unwrap();
    let at = f.dst.join("link");
    std::os::unix::fs::symlink(&target, &at).unwrap();
    // The record says a shortcut was made here; the symbolic link is not it.
    let record = UndoRecord::LinksMade {
        at: vec![(v(&at), LinkKind::Shortcut, v(&target))],
    };
    let report = run(&JobSpec::Undo(record));
    assert_eq!(
        report.failures[0].reason,
        Some(FailureReason::ChangedSinceLinked)
    );
    assert!(std::fs::symlink_metadata(&at).is_ok());
}

#[test]
fn an_undo_skips_a_link_that_is_gone() {
    let f = fx();
    let target = f.src.join("a.txt");
    std::fs::write(&target, "A").unwrap();
    let at = f.dst.join("link");
    let made = run(&link_job(&[(&target, &at, LinkKind::Symlink)]));
    std::fs::remove_file(&at).unwrap();
    let report = run(&JobSpec::Undo(made.undo.unwrap()));
    assert_eq!(report.outcome, Outcome::Completed);
    assert_eq!((report.skipped, report.undo.clone()), (1, None));
}

#[test]
fn a_taken_name_gets_a_number() {
    let f = fx();
    let target = f.src.join("a.txt");
    std::fs::write(&target, "A").unwrap();
    let at = f.dst.join("a.txt");
    std::fs::write(&at, "already here").unwrap();
    let job = link_job(&[(&target, &at, LinkKind::Symlink)]);
    let vfs = Vfs::with_local();
    let env = ExecEnv::new(&vfs, &NonInteractive).naming_os(Os::Other);
    let first = execute(JobId::generate(), &job, &env);
    assert_eq!(first.outcome, Outcome::Completed, "{:?}", first.failures);
    let second = execute(JobId::generate(), &job, &env);
    assert_eq!(
        (first.outputs, second.outputs),
        (
            vec![v(f.dst.join("a (2).txt"))],
            vec![v(f.dst.join("a (3).txt"))]
        )
    );
    // What was there is untouched, and the record names the links as they ended up.
    assert_eq!(std::fs::read_to_string(&at).unwrap(), "already here");
    assert_eq!(
        second.undo,
        Some(UndoRecord::LinksMade {
            at: vec![(v(f.dst.join("a (3).txt")), LinkKind::Symlink, v(&target))]
        })
    );
}

#[test]
fn a_target_that_is_not_there_still_gets_its_link_with_a_warning() {
    let f = fx();
    let target = f.src.join("missing.txt");
    let at = f.dst.join("missing.txt");
    let report = run(&link_job(&[(&target, &at, LinkKind::Symlink)]));
    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
    assert_eq!(report.failures, Vec::new());
    assert_eq!(std::fs::read_link(&at).unwrap(), target);
    let warning = &report.warnings[0];
    assert_eq!(
        (warning.stage, warning.kind, warning.reason.clone()),
        (
            Stage::Create,
            ErrorKind::NotFound,
            Some(FailureReason::LinkTargetMissing)
        )
    );
    assert_eq!(report.outputs, [v(&at)]);
}

#[test]
fn a_kind_this_system_does_not_make_fails_and_makes_nothing() {
    let f = fx();
    let folder = f.src.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let at = f.dst.join("junction");
    let ok = f.dst.join("ok");
    let report = run(&link_job(&[
        (&folder, &at, LinkKind::Junction),
        (&folder, &ok, LinkKind::Symlink),
    ]));
    // The failed link does not stop the next one.
    assert_eq!(report.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (report.failures[0].stage, report.failures[0].kind),
        (Stage::Create, ErrorKind::Unsupported)
    );
    assert!(std::fs::symlink_metadata(&at).is_err());
    assert_eq!(report.outputs, [v(&ok)]);
    // The retry is the link that failed.
    assert_eq!(
        report.retry_spec(),
        Some(link_job(&[(&folder, &at, LinkKind::Junction)]))
    );
}

#[test]
fn a_name_the_disk_cannot_hold_is_refused() {
    let f = fx();
    let target = f.src.join("a.txt");
    std::fs::write(&target, "A").unwrap();
    let at = f.dst.join("n".repeat(300));
    let report = run(&link_job(&[(&target, &at, LinkKind::Symlink)]));
    assert_eq!(report.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (report.failures[0].stage, report.failures[0].kind),
        (Stage::Create, ErrorKind::InvalidPath)
    );
    assert_eq!(std::fs::read_dir(&f.dst).unwrap().count(), 0);
}

#[test]
fn links_run_in_the_quick_lane_and_are_journaled() {
    let spec = link_job(&[]);
    assert_eq!(DefaultLanes.lane(&spec), LaneId::QUICK);
    assert!(spec.journaled());
    let record = UndoRecord::LinksMade { at: Vec::new() };
    assert_eq!(DefaultLanes.lane(&JobSpec::Undo(record)), LaneId::QUICK);
    let record = UndoRecord::LinksRemoved { links: Vec::new() };
    assert_eq!(DefaultLanes.lane(&JobSpec::Undo(record)), LaneId::QUICK);
}

#[test]
fn a_link_in_a_folder_that_is_gone_fails() {
    let f = fx();
    let target = f.src.join("a.txt");
    std::fs::write(&target, "A").unwrap();
    let at = f.root.join("gone").join("link");
    let report = run(&link_job(&[(&target, &at, LinkKind::Symlink)]));
    assert_eq!(report.outcome, Outcome::CompletedWithErrors);
    assert_eq!(report.failures[0].stage, Stage::Create);
    assert_eq!(report.undo, None);
}
