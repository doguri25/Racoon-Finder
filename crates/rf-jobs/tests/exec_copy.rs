#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::collections::BTreeMap;
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use common::*;
use rf_core::ErrorKind;
use rf_jobs::testkit::{temp_files, tree};
use rf_jobs::{
    Control, ExecEnv, ExecEvent, JobId, NonInteractive, Outcome, Stage, Totals, execute,
};
use rf_vfs::Vfs;

#[test]
fn copies_content_and_times_through_a_temp_file() {
    let f = fx();
    let a = f.src.join("a.bin");
    std::fs::write(&a, pattern(3 << 20)).unwrap();
    set_mtime(&a, 1_000_000_000);
    let r = run(&copy(&[a], &f.dst));
    assert_eq!(r.outcome, Outcome::Completed, "{:?}", r.failures);
    let b = f.dst.join("a.bin");
    assert_eq!(std::fs::read(&b).unwrap(), pattern(3 << 20));
    assert_eq!(mtime(&b), at(1_000_000_000));
    let want = Totals {
        files: 1,
        dirs: 0,
        bytes: 3 << 20,
    };
    assert_eq!((r.planned, r.done), (want, want));
    assert_eq!(r.outputs, [v(&b)]);
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
}

#[test]
fn progress_names_the_current_file_and_reaches_the_total() {
    let f = fx();
    let a = f.src.join("a.bin");
    std::fs::write(&a, pattern(3 << 20)).unwrap();
    let seen = Mutex::new(Vec::new());
    let on = |e: ExecEvent| seen.lock().unwrap().push(e);
    let (vfs, res) = (Vfs::with_local(), NonInteractive);
    execute(
        JobId::generate(),
        &copy(std::slice::from_ref(&a), &f.dst),
        &ExecEnv::new(&vfs, &res).on_event(&on),
    );
    let seen = seen.into_inner().unwrap();
    assert!(
        matches!(seen.first(), Some(ExecEvent::Planned { totals, issues: 0 }) if totals.bytes == 3 << 20)
    );
    let progress: Vec<_> = seen
        .iter()
        .filter_map(|e| {
            if let ExecEvent::Progress(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert!(progress.iter().any(|p| p.current == Some(v(&a))));
    let last = progress.last().unwrap();
    assert_eq!(
        (last.done.bytes, last.total.bytes, last.done.files),
        (3 << 20, 3 << 20, 1)
    );
}

#[test]
fn cancel_before_start_writes_nothing() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), b"a").unwrap();
    let control = Control::new();
    control.cancel();
    let (vfs, res) = (Vfs::with_local(), NonInteractive);
    let r = execute(
        JobId::generate(),
        &copy(&[f.src.join("a.txt")], &f.dst),
        &ExecEnv::new(&vfs, &res).control(control),
    );
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

#[test]
fn cancel_mid_file_removes_the_temp_file() {
    let f = fx();
    let a = f.src.join("a.bin");
    std::fs::write(&a, pattern(4 << 20)).unwrap();
    let control = Control::new();
    let c2 = control.clone();
    let on = move |e: ExecEvent| {
        if let ExecEvent::Progress(p) = e {
            if p.done.bytes > 0 {
                c2.cancel();
            }
        }
    };
    let (vfs, res) = (Vfs::with_local(), NonInteractive);
    let r = execute(
        JobId::generate(),
        &copy(std::slice::from_ref(&a), &f.dst),
        &ExecEnv::new(&vfs, &res).control(control).on_event(&on),
    );
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert_eq!(tree(&f.dst), BTreeMap::new(), "{:?}", tree(&f.dst));
    assert_eq!(std::fs::read(&a).unwrap(), pattern(4 << 20));
}

#[test]
fn pause_holds_the_copy_until_resume() {
    let f = fx();
    let a = f.src.join("a.bin");
    std::fs::write(&a, pattern(4 << 20)).unwrap();
    let control = Control::new();
    let (tx, rx) = mpsc::channel();
    let worker = {
        let (control, src, dst) = (control.clone(), a.clone(), f.dst.clone());
        std::thread::spawn(move || {
            let (c2, tx) = (control.clone(), Mutex::new(Some(tx)));
            let on = move |e: ExecEvent| {
                if let ExecEvent::Progress(p) = e {
                    if p.done.bytes > 0 && p.done.bytes < p.total.bytes {
                        if let Some(tx) = tx.lock().unwrap().take() {
                            c2.pause();
                            tx.send(()).unwrap();
                        }
                    }
                }
            };
            let (vfs, res) = (Vfs::with_local(), NonInteractive);
            execute(
                JobId::generate(),
                &copy(&[src], &dst),
                &ExecEnv::new(&vfs, &res).control(control).on_event(&on),
            )
        })
    };
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!f.dst.join("a.bin").exists());
    assert_eq!(temp_files(&f.dst).len(), 1);
    control.resume();
    assert_eq!(worker.join().unwrap().outcome, Outcome::Completed);
    assert_eq!(
        std::fs::read(f.dst.join("a.bin")).unwrap(),
        pattern(4 << 20)
    );
}

#[test]
fn missing_source_is_a_plan_failure() {
    let f = fx();
    let r = run(&copy(&[f.src.join("missing.txt")], &f.dst));
    assert_eq!(r.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (r.failures[0].stage, r.failures[0].kind),
        (Stage::Plan, ErrorKind::NotFound)
    );
    assert_eq!(r.failures[0].source, Some(v(f.src.join("missing.txt"))));
}

#[test]
#[allow(
    clippy::permissions_set_readonly_false,
    reason = "make the temp dir removable"
)]
fn readonly_source_gives_a_readonly_copy() {
    let f = fx();
    let a = f.src.join("ro.txt");
    std::fs::write(&a, b"ro").unwrap();
    let mut perm = std::fs::metadata(&a).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&a, perm.clone()).unwrap();
    let r = run(&copy(std::slice::from_ref(&a), &f.dst));
    assert_eq!(
        (r.outcome, r.warnings.len()),
        (Outcome::Completed, 0),
        "{:?}",
        r.warnings
    );
    let b = f.dst.join("ro.txt");
    assert!(std::fs::metadata(&b).unwrap().permissions().readonly());
    perm.set_readonly(false);
    std::fs::set_permissions(&a, perm.clone()).unwrap();
    std::fs::set_permissions(&b, perm).unwrap();
}

#[cfg(unix)]
#[test]
fn unix_mode_is_copied() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let a = f.src.join("run.sh");
    std::fs::write(&a, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o750)).unwrap();
    assert_eq!(run(&copy(&[a], &f.dst)).outcome, Outcome::Completed);
    assert_eq!(
        std::fs::metadata(f.dst.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o750
    );
}

#[cfg(windows)]
#[test]
fn windows_hidden_attribute_and_streams_are_copied() {
    use std::os::windows::fs::MetadataExt;
    let f = fx();
    let a = f.src.join("h.txt");
    std::fs::write(&a, b"x").unwrap();
    std::fs::write(f.src.join("h.txt:custom"), b"side").unwrap();
    assert!(
        std::process::Command::new("attrib")
            .arg("+h")
            .arg(&a)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(run(&copy(&[a], &f.dst)).outcome, Outcome::Completed);
    assert_eq!(std::fs::read(f.dst.join("h.txt:custom")).unwrap(), b"side");
    assert_ne!(
        std::fs::metadata(f.dst.join("h.txt"))
            .unwrap()
            .file_attributes()
            & 0x2,
        0
    );
}

/// A target that keeps no metadata at all (a provider without `set_metadata`) is nothing to do
/// for each file: the copy says so once, for the first file, and copies every file.
#[test]
fn metadata_the_target_cannot_keep_is_one_warning_per_job() {
    use rf_jobs::testkit::{Fault, FaultyProvider};

    let f = fx();
    let sources: Vec<_> = ["a", "b", "c"]
        .iter()
        .map(|name| {
            let path = f.src.join(name);
            std::fs::write(&path, name).unwrap();
            path
        })
        .collect();
    let p = FaultyProvider::local(vec![Fault::NoMetadata]);
    let r = run_on(&copy(&sources, &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(r.outcome, Outcome::Completed, "{:?}", r.failures);
    assert_eq!(r.done.files, 3);
    let warned: Vec<_> = r.warnings.iter().map(|w| (w.stage, w.kind)).collect();
    assert_eq!(warned, [(Stage::Metadata, ErrorKind::Unsupported)]);
}

/// A file that another program replaces with a FIFO after the plan found it is refused when it
/// is opened, at once: a FIFO without a writer would block a plain open for good.
#[cfg(unix)]
#[test]
fn a_fifo_put_in_the_place_of_a_file_is_refused_at_once() {
    let f = fx();
    let a = f.src.join("a.txt");
    std::fs::write(&a, "A").unwrap();
    let (tx, rx) = mpsc::channel();
    let (job, fifo) = (copy(std::slice::from_ref(&a), &f.dst), a.clone());
    std::thread::spawn(move || {
        let vfs = Vfs::with_local();
        let on = |event: ExecEvent| {
            if matches!(event, ExecEvent::Planned { .. }) {
                std::fs::remove_file(&fifo).unwrap();
                let made = std::process::Command::new("mkfifo").arg(&fifo).status();
                assert!(made.unwrap().success(), "mkfifo failed");
            }
        };
        let env = ExecEnv::new(&vfs, &NonInteractive).on_event(&on);
        let _ = tx.send(execute(JobId::generate(), &job, &env));
    });
    let Ok(r) = rx.recv_timeout(Duration::from_secs(20)) else {
        // Opening the other end lets a blocked open go on, so the thread ends.
        let _ = std::fs::OpenOptions::new().write(true).open(&a);
        panic!("the copy blocked on a FIFO");
    };
    assert_eq!(r.failures.len(), 1, "{:?}", r.failures);
    assert_eq!(
        (r.failures[0].stage, r.failures[0].kind),
        (Stage::Read, ErrorKind::Unsupported)
    );
    assert!(
        r.failures[0].message.ends_with("not a regular file"),
        "{}",
        r.failures[0].message
    );
    assert_eq!(
        r.failures[0].reason,
        Some(rf_jobs::FailureReason::NotARegularFile)
    );
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

/// A link inside a copied folder that is pointed elsewhere after the copy is a change, caught by
/// the link's own time, even when the folder's own time says nothing: the undo leaves the
/// folder where it is.
#[cfg(unix)]
#[test]
fn a_link_pointed_elsewhere_after_a_copy_keeps_its_folder_from_the_undo() {
    use rf_jobs::{FailureReason, JobSpec, UndoRecord};

    let f = fx();
    std::fs::create_dir(f.src.join("dir")).unwrap();
    std::fs::write(f.src.join("dir").join("a.txt"), "A").unwrap();
    std::os::unix::fs::symlink("a.txt", f.src.join("dir").join("link")).unwrap();
    let r = run(&copy(&[f.src.join("dir")], &f.dst));
    assert_eq!(r.outcome, Outcome::Completed, "{:?}", r.failures);
    let Some(UndoRecord::Copied { created, .. }) = r.undo.clone() else {
        panic!("{:?}", r.undo)
    };
    let recorded = created[0].modified.expect("the folder has a time");
    // Past the job's time `at`, also on a clock that ticks coarsely.
    std::thread::sleep(Duration::from_millis(50));
    let link = f.dst.join("dir").join("link");
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("elsewhere", &link).unwrap();
    // The folder's own time is put back, so only the link can tell.
    rf_platform::set_times(&v(f.dst.join("dir")), Some(recorded), None, None).unwrap();
    let undo = run(&JobSpec::Undo(r.undo.unwrap()));
    assert_eq!(undo.failures.len(), 1, "{:?}", undo.failures);
    let failure = &undo.failures[0];
    assert_eq!(
        (
            failure.stage,
            failure.source.clone(),
            failure.reason.clone()
        ),
        (
            Stage::Undo,
            Some(v(f.dst.join("dir"))),
            Some(FailureReason::ChangedSinceCopy)
        )
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::path::Path::new("elsewhere")
    );
    assert_eq!(
        std::fs::read_to_string(f.dst.join("dir").join("a.txt")).unwrap(),
        "A"
    );
}
