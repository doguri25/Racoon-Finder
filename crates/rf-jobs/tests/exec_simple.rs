#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::ffi::OsString;

use common::*;
use rf_core::{ErrorKind, Os};
use rf_jobs::{
    CreateKind, ExecEnv, JobId, JobSpec, NonInteractive, Outcome, Stage, UndoRecord, execute,
};
use rf_vfs::Vfs;

#[test]
fn rename_records_undo() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let rep = run(&JobSpec::Rename {
        from: v(f.src.join("a.txt")),
        to: v(f.src.join("b.txt")),
    });
    assert_eq!(rep.outcome, Outcome::Completed);
    assert_eq!(rep.outputs, [v(f.src.join("b.txt"))]);
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Renamed {
            from: v(f.src.join("a.txt")),
            to: v(f.src.join("b.txt"))
        })
    );
}

#[test]
fn rename_never_overwrites() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    std::fs::write(f.src.join("b.txt"), "B").unwrap();
    let rep = run(&JobSpec::Rename {
        from: v(f.src.join("a.txt")),
        to: v(f.src.join("b.txt")),
    });
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Rename, ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read_to_string(f.src.join("b.txt")).unwrap(), "B");
    assert_eq!(rep.undo, None);
}

#[test]
fn new_folder_takes_the_next_free_name() {
    let f = fx();
    std::fs::create_dir(f.src.join("새 폴더")).unwrap();
    let job = JobSpec::Create {
        parent: v(&f.src),
        name: OsString::from("새 폴더"),
        kind: CreateKind::Dir,
    };
    let (vfs, res) = (Vfs::with_local(), NonInteractive);
    let rep = execute(
        JobId::generate(),
        &job,
        &ExecEnv::new(&vfs, &res).naming_os(Os::Windows),
    );
    let made = v(f.src.join("새 폴더 (2)"));
    assert_eq!(rep.outputs, std::slice::from_ref(&made));
    assert!(f.src.join("새 폴더 (2)").is_dir());
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Created {
            path: made,
            kind: CreateKind::Dir
        })
    );
}

#[test]
fn new_file_is_empty() {
    let f = fx();
    let rep = run(&JobSpec::Create {
        parent: v(&f.src),
        name: OsString::from("새 텍스트.txt"),
        kind: CreateKind::File,
    });
    assert_eq!(rep.outcome, Outcome::Completed);
    assert_eq!(
        std::fs::metadata(f.src.join("새 텍스트.txt"))
            .unwrap()
            .len(),
        0
    );
}

// What follows goes beyond the brief: the rules the controller added for this task.

mod rules {
    use std::collections::BTreeMap;

    use rf_core::{Scheme, VPath};
    use rf_jobs::testkit::{Fault, FaultyProvider, tree};
    use rf_jobs::{FailureReason, IssueKind, Totals};

    use super::*;

    const ONE_FOLDER: Totals = Totals {
        files: 0,
        dirs: 1,
        bytes: 0,
    };

    fn create(parent: &std::path::Path, name: &str, kind: CreateKind) -> JobSpec {
        JobSpec::Create {
            parent: v(parent),
            name: OsString::from(name),
            kind,
        }
    }

    #[test]
    fn a_rename_across_locations_is_refused() {
        let f = fx();
        std::fs::write(f.src.join("a.txt"), "A").unwrap();
        let (from, to) = (
            v(f.src.join("a.txt")),
            VPath::new(Scheme::Remote, "p", &["b.txt"]).unwrap(),
        );
        let rep = run(&JobSpec::Rename {
            from: from.clone(),
            to: to.clone(),
        });
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.message.clone()),
            (
                Stage::Rename,
                ErrorKind::Unsupported,
                rf_core::Error::Unsupported(format!(
                    "cannot rename across locations: {from} -> {to}"
                ))
                .to_string()
            )
        );
        assert_eq!(failure.reason, Some(FailureReason::AcrossLocations));
        assert_eq!(std::fs::read_to_string(f.src.join("a.txt")).unwrap(), "A");
        assert_eq!((rep.outputs, rep.undo), (Vec::new(), None));
    }

    #[test]
    fn a_renamed_folder_is_planned_and_counted_as_one_folder() {
        let f = fx();
        rf_jobs::testkit::write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
        let before = tree(&f.src.join("dir"));
        let rep = run(&JobSpec::Rename {
            from: v(f.src.join("dir")),
            to: v(f.src.join("moved")),
        });
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!((rep.planned, rep.done), (ONE_FOLDER, ONE_FOLDER));
        assert_eq!(tree(&f.src.join("moved")), before);
        assert!(!f.src.join("dir").exists());
    }

    #[test]
    fn renaming_a_missing_item_fails_its_plan() {
        let f = fx();
        let rep = run(&JobSpec::Rename {
            from: v(f.src.join("missing")),
            to: v(f.src.join("b.txt")),
        });
        assert_eq!(
            (rep.failures[0].stage, rep.failures[0].kind),
            (Stage::Plan, ErrorKind::NotFound)
        );
        assert_eq!(
            (rep.planned, rep.done),
            (Totals::default(), Totals::default())
        );
        assert_eq!(tree(&f.src), BTreeMap::new());
    }

    #[test]
    fn a_new_folder_is_planned_and_counted_as_one_folder() {
        let f = fx();
        let rep = run(&create(&f.src, "new", CreateKind::Dir));
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!((rep.planned, rep.done), (ONE_FOLDER, ONE_FOLDER));
        assert!(f.src.join("new").is_dir());
    }

    /// A name whose `stat` fails for another reason than `NotFound` may well be taken: the
    /// job fails rather than make anything under it or under a number.
    #[test]
    fn a_name_that_cannot_be_examined_is_not_taken_for_free() {
        let f = fx();
        let p = FaultyProvider::local(vec![Fault::FailStat { name: "x".into() }]);
        let rep = run_on(
            &create(&f.src, "x", CreateKind::File),
            &p.vfs(),
            &NonInteractive,
        );
        assert_eq!(
            (rep.failures[0].stage, rep.failures[0].kind),
            (Stage::Create, ErrorKind::PermissionDenied)
        );
        assert_eq!(tree(&f.src), BTreeMap::new());
        assert_eq!(rep.undo, None);
    }

    /// A numbered name can be longer than the file system takes: the job fails with that
    /// issue, before the file system is asked, and never with a raw OS error.
    #[test]
    fn a_numbered_name_too_long_for_the_file_system_fails_with_its_issue() {
        let f = fx();
        let name = "n".repeat(253);
        std::fs::write(f.src.join(&name), "").unwrap();
        let (vfs, res) = (Vfs::with_local(), NonInteractive);
        let job = create(&f.src, &name, CreateKind::File);
        let env = ExecEnv::new(&vfs, &res).naming_os(Os::Windows);
        let rep = execute(JobId::generate(), &job, &env);
        let failure = &rep.failures[0];
        // ` (2)` takes the name to 257 units.
        let issue = IssueKind::NameTooLong { len: 257, max: 255 };
        assert_eq!(
            (failure.stage, failure.kind, failure.message.clone()),
            (Stage::Create, ErrorKind::InvalidPath, issue.to_string())
        );
        assert_eq!(failure.target, Some(v(f.src.join(format!("{name} (2)")))));
        assert_eq!(tree(&f.src).len(), 1);
    }

    /// A name the target file system cannot hold, or that Windows would not open, is never
    /// made: the job fails with the issue.
    #[test]
    fn a_name_the_file_system_cannot_hold_is_never_made() {
        let f = fx();
        let p = FaultyProvider::local(vec![Fault::FsName("NTFS".into())]);
        let cases = [
            ("a?b", IssueKind::InvalidCharacter('?')),
            ("CON", IssueKind::ReservedName),
            ("end.", IssueKind::TrailingDotOrSpace),
            ("CONIN$", IssueKind::ReservedName),
            ("com\u{b9}.txt", IssueKind::ReservedName),
        ];
        for (name, issue) in cases {
            let rep = run_on(
                &create(&f.src, name, CreateKind::Dir),
                &p.vfs(),
                &NonInteractive,
            );
            let failure = &rep.failures[0];
            assert_eq!(
                (failure.stage, failure.kind, failure.message.clone()),
                (Stage::Create, ErrorKind::InvalidPath, issue.to_string()),
                "{name}"
            );
            assert_eq!(failure.reason, Some(FailureReason::Issue(issue)), "{name}");
        }
        assert_eq!(tree(&f.src), BTreeMap::new());
    }

    /// A rename never gives an item a name its file system cannot hold or that Windows would
    /// not open: the job fails with the issue and the item keeps its name.
    #[test]
    fn a_new_name_the_file_system_cannot_hold_is_refused() {
        let f = fx();
        std::fs::write(f.src.join("a.txt"), "A").unwrap();
        let before = tree(&f.src);
        let p = FaultyProvider::local(vec![Fault::FsName("NTFS".into())]);
        let cases = [
            ("CON", IssueKind::ReservedName),
            ("a.", IssueKind::TrailingDotOrSpace),
            ("a|b.txt", IssueKind::InvalidCharacter('|')),
            ("CONOUT$.log", IssueKind::ReservedName),
            ("LPT\u{b3}", IssueKind::ReservedName),
        ];
        for (name, issue) in cases {
            let job = JobSpec::Rename {
                from: v(f.src.join("a.txt")),
                to: v(f.src.join(name)),
            };
            let rep = run_on(&job, &p.vfs(), &NonInteractive);
            let failure = &rep.failures[0];
            assert_eq!(
                (failure.stage, failure.kind, failure.message.clone()),
                (Stage::Rename, ErrorKind::InvalidPath, issue.to_string()),
                "{name}"
            );
            assert_eq!((rep.outputs.len(), rep.undo), (0, None), "{name}");
        }
        assert_eq!(tree(&f.src), before);
    }
}
