#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::*;
use rf_jobs::testkit::{Fault, FaultyProvider, Node, temp_files, tree};
use rf_jobs::{
    ConflictPolicy, Control, ExecEnv, ExecEvent, JobId, NonInteractive, Outcome, Stage,
    TransferMode, TransferOptions, execute,
};

const MIB: u64 = 1 << 20;

fn five_files(f: &Fx) -> Vec<PathBuf> {
    (0..5u64)
        .map(|i| {
            let p = f.src.join(format!("f{i}.bin"));
            std::fs::write(&p, pattern(2 * MIB + i)).unwrap();
            p
        })
        .collect()
}

/// The copy invariant at the target: no temporary file, and every file there is a byte-exact
/// copy of the source file of the same name.
fn assert_no_partial(f: &Fx) {
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
    for (name, node) in tree(&f.dst) {
        assert_eq!(
            node,
            Node::File(std::fs::read(f.src.join(&name)).unwrap()),
            "{name}"
        );
    }
}

/// The move invariant: no temporary file at the target, and each original file is in exactly
/// one place, the source or the target, with its original content.
fn assert_one_place(f: &Fx, originals: &BTreeMap<String, Node>) {
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
    for (name, node) in originals {
        let (s, d) = (f.src.join(name), f.dst.join(name));
        assert!(
            s.exists() != d.exists(),
            "{name} must be in exactly one place"
        );
        let place = if s.exists() { s } else { d };
        assert_eq!(&Node::File(std::fs::read(place).unwrap()), node, "{name}");
    }
}

#[test]
fn a_write_failure_never_leaves_a_partial_file() {
    let f = fx();
    let srcs = five_files(&f);
    let before = tree(&f.src);
    let p = FaultyProvider::local(vec![Fault::FailWrite {
        file: 2,
        after: MIB,
    }]);
    let rep = run_on(&copy(&srcs, &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (rep.failures.len(), rep.failures[0].stage),
        (1, Stage::Write)
    );
    assert_eq!(tree(&f.dst).len(), 4);
    assert_no_partial(&f);
    assert_eq!(tree(&f.src), before);
}

#[test]
fn a_full_disk_during_a_move_never_deletes_the_source() {
    let f = fx();
    std::fs::write(f.src.join("a.bin"), pattern(2 * MIB)).unwrap();
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::DiskFull {
            file: 0,
            after: MIB / 2,
        },
    ]);
    let job = spec(
        TransferMode::Move,
        &[f.src.join("a.bin")],
        &f.dst,
        TransferOptions::default(),
    );
    let rep = run_on(&job, &p.vfs(), &NonInteractive);
    assert_eq!(rep.failures[0].stage, Stage::Write);
    assert!(
        rep.failures[0].message.contains("injected disk full"),
        "{}",
        rep.failures[0].message
    );
    assert_eq!(
        std::fs::read(f.src.join("a.bin")).unwrap(),
        pattern(2 * MIB)
    );
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

#[test]
fn an_fsync_failure_leaves_no_file() {
    let f = fx();
    std::fs::write(f.src.join("a.bin"), pattern(MIB)).unwrap();
    let p = FaultyProvider::local(vec![Fault::FailFinish { file: 0 }]);
    let rep = run_on(
        &copy(&[f.src.join("a.bin")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(rep.failures[0].stage, Stage::Write);
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

#[test]
fn a_commit_failure_cleans_the_temp_file() {
    let f = fx();
    std::fs::write(f.src.join("a.bin"), pattern(MIB)).unwrap();
    let p = FaultyProvider::local(vec![Fault::FailRename { call: 0 }]);
    let rep = run_on(
        &copy(&[f.src.join("a.bin")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(rep.failures[0].stage, Stage::Commit);
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

#[test]
fn a_failed_overwrite_keeps_the_old_content() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), pattern(MIB)).unwrap();
    std::fs::write(f.dst.join("a.txt"), "old").unwrap();
    let p = FaultyProvider::local(vec![Fault::FailWrite { file: 0, after: 10 }]);
    let opts = TransferOptions {
        conflict: ConflictPolicy::Overwrite,
        ..TransferOptions::default()
    };
    run_on(
        &spec(TransferMode::Copy, &[f.src.join("a.txt")], &f.dst, opts),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(std::fs::read_to_string(f.dst.join("a.txt")).unwrap(), "old");
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
}

#[test]
fn cancelling_a_move_keeps_every_file_in_one_place() {
    let f = fx();
    let srcs = five_files(&f);
    let originals = tree(&f.src);
    let p = FaultyProvider::local(vec![Fault::CrossDevice]);
    let control = Control::new();
    let c2 = control.clone();
    let on = move |e: ExecEvent| {
        if let ExecEvent::Progress(pr) = e {
            if pr.done.files >= 1 {
                c2.cancel();
            }
        }
    };
    let (vfs, res) = (p.vfs(), NonInteractive);
    let job = spec(
        TransferMode::Move,
        &srcs,
        &f.dst,
        TransferOptions::default(),
    );
    let rep = execute(
        JobId::generate(),
        &job,
        &ExecEnv::new(&vfs, &res).control(control).on_event(&on),
    );
    assert_eq!(rep.outcome, Outcome::Cancelled);
    assert_one_place(&f, &originals);
}

#[test]
fn a_panicking_provider_is_contained() {
    let f = fx();
    std::fs::write(f.src.join("a.bin"), pattern(MIB)).unwrap();
    let p = FaultyProvider::local(vec![Fault::Panic { file: 0 }]);
    let rep = run_on(
        &copy(&[f.src.join("a.bin")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(
        (rep.outcome, rep.failures[0].stage),
        (Outcome::Aborted, Stage::Internal)
    );
    assert!(rep.failures[0].message.contains("injected panic"));
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(24))]
    #[test]
    fn random_faults_never_lose_data(
        kind in 0usize..5,
        file in 0usize..3,
        after in 0..(3 * MIB),
        moving in proptest::prelude::any::<bool>(),
    ) {
        let f = fx();
        let srcs: Vec<PathBuf> = (0..3u64)
            .map(|i| {
                let p = f.src.join(format!("f{i}"));
                std::fs::write(&p, pattern(MIB + 7 * i)).unwrap();
                p
            })
            .collect();
        let originals = tree(&f.src);
        let fault = match kind {
            0 => Fault::FailWrite { file, after },
            1 => Fault::DiskFull { file, after },
            2 => Fault::FailFinish { file },
            3 => Fault::FailRename { call: file },
            _ => Fault::Panic { file },
        };
        let mut faults = vec![fault];
        if moving {
            faults.push(Fault::CrossDevice);
        }
        let p = FaultyProvider::local(faults);
        let mode = if moving { TransferMode::Move } else { TransferMode::Copy };
        run_on(&spec(mode, &srcs, &f.dst, TransferOptions::default()), &p.vfs(), &NonInteractive);
        if moving {
            assert_one_place(&f, &originals);
        } else {
            assert_eq!(tree(&f.src), originals);
            assert_no_partial(&f);
        }
    }
}
