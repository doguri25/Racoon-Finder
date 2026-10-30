//! Deleting for good the copies of a copy job (OPS-12): a copy goes only while it is as its
//! copy job left it and its original still holds everything it holds, looked at right before
//! it goes, also after a crash. And the look itself: a copy beside what its job left, and an
//! original beside its copy.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

use common::*;
use rf_jobs::testkit::{Fault, FaultyProvider, wait_finished, wait_for};
use rf_jobs::{
    Compared, CopyFound, CopyOf, CreatedItem, DeleteCopiesSpec, EngineConfig, FailureReason,
    IncompleteJob, JobEngine, JobEvent, JobKind, JobReport, JobSpec, NonInteractive, Outcome,
    Stage, UndoRecord, check_copy, compare_with_made, compare_with_original,
};
use rf_vfs::{CancelToken, LocalProvider, Provider, Vfs};

const WAIT: Duration = Duration::from_secs(30);

/// Gives the file at `path` the modification time `at`.
fn set_modified(path: &Path, at: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(at))
        .unwrap();
}

/// Writes `text` at `path`, changed at `at`, as a copy keeps the time of its source.
fn write_at(path: &Path, text: &str, at: SystemTime) {
    std::fs::write(path, text).unwrap();
    set_modified(path, at);
}

/// What a copy job records of the item at `path` it made: what `stat` finds there now.
fn made_at(path: &Path) -> CreatedItem {
    let path = v(path);
    let entry = LocalProvider::new().stat(&path).unwrap();
    CreatedItem::from_entry(path, &entry)
}

/// Makes `link` a symbolic link to the file or folder `target` (which Windows lets only a user
/// with the privilege, or in developer mode, do).
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let made = if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    made
}

#[test]
fn a_copy_is_compared_with_its_original() {
    let dir = tempfile::TempDir::new().unwrap();
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for folder in [&src, &dst] {
        std::fs::create_dir_all(folder.join("docs").join("sub")).unwrap();
        write_at(&folder.join("a.txt"), "AAAA", at);
        write_at(&folder.join("docs").join("x.txt"), "X", at);
        write_at(&folder.join("docs").join("sub").join("y.txt"), "YY", at);
    }
    write_at(&src.join("docs").join("only-in-source.txt"), "S", at);
    let vfs = Vfs::with_local();
    let go_on = CancelToken::new();
    let compare =
        |name: &str| compare_with_original(&vfs, &v(dst.join(name)), &v(src.join(name)), &go_on);

    // As they were: a file of the same size and time, and a folder that holds everything the
    // copy holds (and more).
    assert_eq!(compare("a.txt"), Compared::Same);
    assert_eq!(compare("docs"), Compared::Same);

    // Something of the copy is not in the original: deleting the copy would lose it.
    std::fs::remove_file(src.join("docs").join("sub").join("y.txt")).unwrap();
    assert_eq!(compare("docs"), Compared::Changed);
    write_at(&src.join("docs").join("sub").join("y.txt"), "YYY", at);
    assert_eq!(compare("docs"), Compared::Changed, "another size deep down");
    write_at(&src.join("docs").join("sub").join("y.txt"), "YY", at);
    assert_eq!(compare("docs"), Compared::Same);

    // The original edited since (same size, a later time), of another kind, or gone.
    set_modified(&src.join("a.txt"), at + Duration::from_secs(60));
    assert_eq!(compare("a.txt"), Compared::Changed);
    std::fs::remove_file(src.join("a.txt")).unwrap();
    std::fs::create_dir(src.join("a.txt")).unwrap();
    assert_eq!(compare("a.txt"), Compared::Changed);
    std::fs::remove_dir(src.join("a.txt")).unwrap();
    assert_eq!(compare("a.txt"), Compared::Missing);

    // A copy that is gone cannot be told about.
    std::fs::remove_file(dst.join("docs").join("x.txt")).unwrap();
    let (copy, original) = (
        dst.join("docs").join("x.txt"),
        src.join("docs").join("x.txt"),
    );
    assert_eq!(
        compare_with_original(&vfs, &v(copy), &v(original), &go_on),
        Compared::Unknown
    );

    // Once the look is stopped, it cannot be told.
    let stopped = CancelToken::new();
    stopped.cancel();
    assert_eq!(compare("docs"), Compared::Same);
    assert_eq!(
        compare_with_original(&vfs, &v(dst.join("docs")), &v(src.join("docs")), &stopped),
        Compared::Unknown
    );
}

#[test]
fn a_copy_is_checked_against_what_the_copy_left() {
    let dir = tempfile::TempDir::new().unwrap();
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for folder in [&src, &dst] {
        std::fs::create_dir_all(folder.join("docs").join("sub")).unwrap();
        write_at(&folder.join("a.txt"), "AAAA", old);
        write_at(&folder.join("docs").join("sub").join("y.txt"), "YY", old);
    }
    // The copy job ended after it wrote everything (later than anything here).
    let ended = SystemTime::now() + Duration::from_secs(3600);
    let (file, folder) = (made_at(&dst.join("a.txt")), made_at(&dst.join("docs")));
    let vfs = Vfs::with_local();
    let go_on = CancelToken::new();
    let made = |item: &CreatedItem| compare_with_made(&vfs, item, Some(ended), &go_on);

    // As the copy left them.
    assert_eq!(made(&file), Compared::Same);
    assert_eq!(made(&folder), Compared::Same);
    let original = v(src.join("a.txt"));
    assert_eq!(
        check_copy(&vfs, &file, Some(ended), &original, &go_on),
        CopyFound::SAME
    );

    // A file of the same size written since, or of another size.
    set_modified(&dst.join("a.txt"), old + Duration::from_secs(60));
    assert_eq!(made(&file), Compared::Changed);
    set_modified(&dst.join("a.txt"), old);
    assert_eq!(made(&file), Compared::Same);
    write_at(&dst.join("a.txt"), "AAAAA", old);
    assert_eq!(made(&file), Compared::Changed);
    // The copy is not compared with its original then: it is not the copy any more.
    assert_eq!(
        check_copy(&vfs, &file, Some(ended), &original, &go_on),
        CopyFound {
            copy: Compared::Changed,
            original: Compared::Unknown,
        }
    );
    std::fs::remove_file(dst.join("a.txt")).unwrap();
    assert_eq!(made(&file), Compared::Missing);

    // A folder with something deep down newer than the job's end, or without the job's time.
    let deep = dst.join("docs").join("sub").join("y.txt");
    set_modified(&deep, ended + Duration::from_secs(60));
    assert_eq!(made(&folder), Compared::Changed);
    set_modified(&deep, old);
    assert_eq!(made(&folder), Compared::Same);
    assert_eq!(
        compare_with_made(&vfs, &folder, None, &go_on),
        Compared::Unknown
    );

    // Once the look is stopped, it cannot be told.
    let stopped = CancelToken::new();
    stopped.cancel();
    assert_eq!(
        compare_with_made(&vfs, &folder, Some(ended), &stopped),
        Compared::Unknown
    );
}

#[test]
fn an_original_that_is_the_copy_itself_cannot_be_told() {
    let dir = tempfile::TempDir::new().unwrap();
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    std::fs::create_dir_all(dst.join("docs")).unwrap();
    std::fs::create_dir(&src).unwrap();
    write_at(&dst.join("a.txt"), "AAAA", at);
    write_at(&dst.join("docs").join("x.txt"), "X", at);
    let vfs = Vfs::with_local();
    let go_on = CancelToken::new();
    let compare =
        |copy: &Path, original: &Path| compare_with_original(&vfs, &v(copy), &v(original), &go_on);

    // A file of the copy's size and time elsewhere is a copy as it was.
    write_at(&dir.path().join("real.txt"), "AAAA", at);
    assert_eq!(
        compare(&dst.join("a.txt"), &dir.path().join("real.txt")),
        Compared::Same
    );

    // A hard link is the copy under another name: it cannot be told.
    std::fs::hard_link(dst.join("a.txt"), dir.path().join("hard.txt")).unwrap();
    assert_eq!(
        compare(&dst.join("a.txt"), &dir.path().join("hard.txt")),
        Compared::Unknown
    );

    // An original that is a link to its copy: deleting the copy would lose the only data.
    match symlink(&dst.join("a.txt"), &src.join("a.txt")) {
        Ok(()) => {}
        Err(e) if cfg!(windows) => {
            eprintln!("skipped: cannot create symlinks here: {e}");
            return;
        }
        Err(e) => panic!("a link: {e}"),
    }
    assert_eq!(
        compare(&dst.join("a.txt"), &src.join("a.txt")),
        Compared::Unknown
    );
    symlink(&dst.join("docs"), &src.join("docs")).unwrap();
    assert_eq!(
        compare(&dst.join("docs"), &src.join("docs")),
        Compared::Unknown
    );
    // Nothing would be deleted then, though the copy is as its job left it.
    assert_eq!(
        check_copy(
            &vfs,
            &made_at(&dst.join("a.txt")),
            None,
            &v(src.join("a.txt")),
            &go_on
        ),
        CopyFound {
            copy: Compared::Same,
            original: Compared::Unknown,
        }
    );

    // Reached through a linked folder, the original is the copy itself.
    let through = dir.path().join("through");
    symlink(&dst, &through).unwrap();
    assert_eq!(
        compare(&dst.join("a.txt"), &through.join("a.txt")),
        Compared::Unknown
    );
    assert_eq!(
        compare(&dst.join("docs"), &through.join("docs")),
        Compared::Unknown
    );

    // A folder whose item leads to the copy's own item, deep down.
    let holder = dir.path().join("holder");
    std::fs::create_dir(&holder).unwrap();
    symlink(&dst.join("docs").join("x.txt"), &holder.join("x.txt")).unwrap();
    assert_eq!(compare(&dst.join("docs"), &holder), Compared::Unknown);
}

/// Copies `names` from `src` into `dst` and gives the delete of those copies, each with its
/// original, as the window builds it from the copy's record.
fn copied(f: &Fx, names: &[&str]) -> DeleteCopiesSpec {
    let sources: Vec<PathBuf> = names.iter().map(|name| f.src.join(name)).collect();
    let report = run(&copy(&sources, &f.dst));
    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
    let Some(UndoRecord::Copied { created, at, .. }) = report.undo.clone() else {
        panic!("a copy's record: {:?}", report.undo);
    };
    let copies = created
        .into_iter()
        .map(|made| {
            let name = made.path.as_local().unwrap().file_name().unwrap();
            let original = v(f.src.join(name));
            CopyOf { made, original }
        })
        .collect();
    DeleteCopiesSpec { copies, at }
}

/// Two files and a folder in `src`, and the delete of their copies in `dst`.
fn three_copies(f: &Fx) -> DeleteCopiesSpec {
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    std::fs::write(f.src.join("b.txt"), "BB").unwrap();
    std::fs::create_dir_all(f.src.join("docs").join("sub")).unwrap();
    std::fs::write(f.src.join("docs").join("sub").join("x.txt"), "X").unwrap();
    copied(f, &["a.txt", "b.txt", "docs"])
}

/// The copies the failures of `report` kept, each with how it and its original were found.
fn kept(report: &JobReport) -> Vec<(PathBuf, CopyFound)> {
    report
        .failures
        .iter()
        .map(|failure| match &failure.reason {
            Some(FailureReason::CopyKept(found)) => {
                assert_eq!(failure.stage, Stage::Delete);
                assert_eq!(failure.kind, rf_core::ErrorKind::Unsupported);
                let path = failure.source.as_ref().unwrap().as_local().unwrap();
                (path.to_path_buf(), *found)
            }
            other => panic!("a kept copy, not {other:?}: {failure:?}"),
        })
        .collect()
}

#[test]
fn copies_as_they_were_go_for_good_and_their_originals_stay() {
    let f = fx();
    let spec = three_copies(&f);
    let job = JobSpec::DeleteCopies(spec);
    assert_eq!(job.kind(), JobKind::Delete);
    assert!(job.journaled());
    let report = run(&job);
    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
    assert!(!f.dst.join("a.txt").exists());
    assert!(!f.dst.join("b.txt").exists());
    assert!(!f.dst.join("docs").exists());
    assert_eq!(std::fs::read_to_string(f.src.join("b.txt")).unwrap(), "BB");
    assert!(f.src.join("docs").join("sub").join("x.txt").is_file());
    // Everything in the folder is counted and done: two files, two folders, and two more files.
    assert_eq!((report.done.files, report.done.dirs), (3, 2));
    assert_eq!(report.planned.files + report.planned.dirs, 5);
    // A delete for good: nothing brings them back.
    assert_eq!(report.undo, None);
    assert_eq!(report.retry_spec(), None);
}

#[test]
fn a_copy_changed_between_submit_and_run_is_kept() {
    let f = fx();
    let spec = three_copies(&f);
    let elsewhere = f.root.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(f.root.join("hold.bin"), "H").unwrap();
    // A copy holds the main lane, and the delete of the copies waits behind it.
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, rx) = JobEngine::start(EngineConfig::new(p.vfs(), Arc::new(NonInteractive)));
    e.submit(copy(&[f.root.join("hold.bin")], &elsewhere));
    assert!(p.wait_held(WAIT));
    let id = e.submit(JobSpec::DeleteCopies(spec.clone()));

    // Meanwhile a file of the copies is written to, and something is put in the copied folder.
    std::fs::write(f.dst.join("b.txt"), "written since").unwrap();
    std::fs::write(f.dst.join("docs").join("new.txt"), "N").unwrap();
    p.release();
    let report = wait_finished(&rx, id, WAIT);

    assert_eq!(report.outcome, Outcome::CompletedWithErrors);
    let changed = CopyFound {
        copy: Compared::Changed,
        original: Compared::Unknown,
    };
    assert_eq!(
        kept(&report),
        [
            (f.dst.join("b.txt"), changed),
            (f.dst.join("docs"), changed)
        ]
    );
    assert!(!f.dst.join("a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.dst.join("b.txt")).unwrap(),
        "written since"
    );
    assert!(f.dst.join("docs").join("new.txt").is_file());
    assert!(f.dst.join("docs").join("sub").join("x.txt").is_file());
    // A retry looks at the kept copies again, as they were copied.
    let Some(JobSpec::DeleteCopies(retry)) = report.retry_spec() else {
        panic!("a retry: {:?}", report.retry_spec());
    };
    assert_eq!(retry.copies, spec.copies[1..]);
    assert_eq!(retry.at, spec.at);
    assert!(e.shutdown(WAIT));
}

#[test]
fn a_copy_whose_original_changed_or_went_is_kept() {
    let f = fx();
    let spec = three_copies(&f);
    // An original written to (another size), and an item gone from the copied folder's
    // original: deleting either copy would lose what only it holds now.
    std::fs::write(f.src.join("a.txt"), "AAAA").unwrap();
    std::fs::remove_file(f.src.join("docs").join("sub").join("x.txt")).unwrap();
    let report = run(&JobSpec::DeleteCopies(spec));
    assert_eq!(report.outcome, Outcome::CompletedWithErrors);
    let original = |original| CopyFound {
        copy: Compared::Same,
        original,
    };
    assert_eq!(
        kept(&report),
        [
            (f.dst.join("a.txt"), original(Compared::Changed)),
            (f.dst.join("docs"), original(Compared::Changed)),
        ]
    );
    assert_eq!(std::fs::read_to_string(f.dst.join("a.txt")).unwrap(), "A");
    assert!(f.dst.join("docs").join("sub").join("x.txt").is_file());
    assert!(!f.dst.join("b.txt").exists());

    // An original gone altogether.
    let f = fx();
    let spec = three_copies(&f);
    std::fs::remove_file(f.src.join("b.txt")).unwrap();
    let report = run(&JobSpec::DeleteCopies(spec));
    assert_eq!(
        kept(&report),
        [(f.dst.join("b.txt"), original(Compared::Missing))]
    );
    assert!(f.dst.join("b.txt").is_file());
}

#[test]
fn an_original_that_is_the_copy_itself_keeps_the_copy() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let spec = copied(&f, &["a.txt"]);
    // The original is replaced by a hard link to its copy: one item under two names, so the
    // two cannot be told apart and the copy stays.
    std::fs::remove_file(f.src.join("a.txt")).unwrap();
    std::fs::hard_link(f.dst.join("a.txt"), f.src.join("a.txt")).unwrap();
    let report = run(&JobSpec::DeleteCopies(spec));
    assert_eq!(
        kept(&report),
        [(
            f.dst.join("a.txt"),
            CopyFound {
                copy: Compared::Same,
                original: Compared::Unknown,
            }
        )]
    );
    assert_eq!(std::fs::read_to_string(f.dst.join("a.txt")).unwrap(), "A");
}

#[test]
fn a_copy_that_is_gone_is_skipped() {
    let f = fx();
    let spec = three_copies(&f);
    std::fs::remove_file(f.dst.join("a.txt")).unwrap();
    let report = run(&JobSpec::DeleteCopies(spec));
    assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
    assert_eq!(report.skipped, 1);
    assert!(!f.dst.join("b.txt").exists() && !f.dst.join("docs").exists());
}

fn restart(journal: &Path) -> (JobEngine, Receiver<JobEvent>, Vec<IncompleteJob>) {
    let (e, rx) = JobEngine::start(
        EngineConfig::new(Vfs::with_local(), Arc::new(NonInteractive)).with_journal(journal),
    );
    match wait_for(&rx, WAIT, |ev| matches!(ev, JobEvent::Recovered { .. })) {
        JobEvent::Recovered { jobs, .. } => (e, rx, jobs),
        _ => unreachable!(),
    }
}

#[test]
fn a_recovered_delete_of_copies_looks_at_them_again() {
    let f = fx();
    let journal = f.root.join("state");
    let spec = three_copies(&f);
    let elsewhere = f.root.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(f.root.join("hold.bin"), "H").unwrap();
    // The delete of the copies is queued behind a copy when the app ends.
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, _rx) = JobEngine::start(
        EngineConfig::new(p.vfs(), Arc::new(NonInteractive)).with_journal(&journal),
    );
    e.submit(copy(&[f.root.join("hold.bin")], &elsewhere));
    assert!(p.wait_held(WAIT));
    let id = e.submit(JobSpec::DeleteCopies(spec.clone()));
    let releaser = {
        let p = Arc::clone(&p);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            p.release();
        })
    };
    assert!(e.shutdown(WAIT));
    releaser.join().unwrap();
    assert!(f.dst.join("a.txt").exists(), "it never ran");

    // Before the next start, a copy is written to.
    std::fs::write(f.dst.join("a.txt"), "written since").unwrap();
    let (e, rx, jobs) = restart(&journal);
    let job = jobs
        .iter()
        .find(|job| job.id == id)
        .expect("it is recovered");
    assert_eq!(job.spec, JobSpec::DeleteCopies(spec));
    let again = e.resubmit_recovered(id).expect("resubmitted");
    let report = wait_finished(&rx, again, WAIT);

    // It looks at each copy again before it deletes it: the one written to stays.
    assert_eq!(
        kept(&report),
        [(
            f.dst.join("a.txt"),
            CopyFound {
                copy: Compared::Changed,
                original: Compared::Unknown,
            }
        )]
    );
    assert_eq!(
        std::fs::read_to_string(f.dst.join("a.txt")).unwrap(),
        "written since"
    );
    assert!(!f.dst.join("b.txt").exists() && !f.dst.join("docs").exists());
    assert!(e.shutdown(WAIT));
}
