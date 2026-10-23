#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::collections::BTreeMap;

use common::*;
use rf_core::{ErrorKind, Os};
use rf_jobs::testkit::{Fault, FaultyProvider, ScriptedResolver, tree, write_tree};
use rf_jobs::{
    ConflictPolicy, ExecEnv, IssueKind, JobId, JobSpec, NonInteractive, Outcome, PreflightAnswer,
    Stage, TransferMode, TransferOptions, execute,
};
use rf_vfs::testkit::UNICODE_NAMES;
use rf_vfs::{DiskSpace, Vfs};

#[test]
fn copies_a_nested_tree_with_unicode_and_long_paths() {
    let f = fx();
    let mut files: Vec<(String, String)> = UNICODE_NAMES
        .iter()
        .enumerate()
        .map(|(i, n)| (format!("dir/u{i}/{n}"), (*n).to_string()))
        .collect();
    let deep = (0..6)
        .map(|i| format!("긴 경로 segment {i} {}", "x".repeat(40)))
        .collect::<Vec<_>>()
        .join("/");
    files.push((format!("dir/{deep}/끝.txt"), "deep".into()));
    let entries: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    write_tree(&f.src, &entries);
    let r = run(&copy(&[f.src.join("dir")], &f.dst));
    assert_eq!(r.outcome, Outcome::Completed, "{:?}", r.failures);
    let copied = f.dst.join("dir").join(&deep).join("끝.txt");
    assert!(copied.to_string_lossy().encode_utf16().count() > 300);
    assert_eq!(tree(&f.dst.join("dir")), tree(&f.src.join("dir")));
    assert_eq!(r.planned, r.done);
    assert_eq!(
        r.done.files,
        u64::try_from(UNICODE_NAMES.len()).unwrap() + 1
    );
}

#[test]
fn merges_into_an_existing_folder_without_asking() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    write_tree(&f.dst, &[("dir/other.txt", "O")]);
    let r = ScriptedResolver::new();
    let rep = run_on(&copy(&[f.src.join("dir")], &f.dst), &Vfs::with_local(), &r);
    assert_eq!(rep.outcome, Outcome::Completed);
    assert_eq!(
        tree(&f.dst.join("dir")).into_keys().collect::<Vec<_>>(),
        ["a.txt", "other.txt"]
    );
    assert_eq!(r.calls(), Vec::new());
}

#[test]
fn empty_folders_are_copied_with_their_times() {
    let f = fx();
    write_tree(&f.src, &[("dir/empty/", "")]);
    set_mtime(&f.src.join("dir").join("empty"), 1_000_000_000);
    set_mtime(&f.src.join("dir"), 1_100_000_000);
    assert_eq!(
        run(&copy(&[f.src.join("dir")], &f.dst)).outcome,
        Outcome::Completed
    );
    assert_eq!(mtime(&f.dst.join("dir").join("empty")), at(1_000_000_000));
    assert_eq!(mtime(&f.dst.join("dir")), at(1_100_000_000));
}

#[test]
fn copying_a_folder_into_itself_is_refused() {
    let f = fx();
    write_tree(&f.src, &[("dir/sub/", "")]);
    let rep = run(&copy(&[f.src.join("dir")], &f.src.join("dir").join("sub")));
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Plan, ErrorKind::Unsupported)
    );
    assert_eq!(tree(&f.src.join("dir").join("sub")), BTreeMap::new());
}

#[test]
fn missing_space_is_reported_before_writing() {
    let f = fx();
    write_tree(&f.src, &[("a.txt", "0123456789")]);
    let p = FaultyProvider::local(vec![Fault::Space(DiskSpace {
        total: 100,
        free: 4,
        available: 4,
    })]);
    let r = ScriptedResolver::new().on_preflight(PreflightAnswer::Cancel);
    let rep = run_on(&copy(&[f.src.join("a.txt")], &f.dst), &p.vfs(), &r);
    assert_eq!(rep.outcome, Outcome::Cancelled);
    assert_eq!(
        rep.issues[0].kind,
        IssueKind::NotEnoughSpace {
            needed: 10,
            available: 4
        }
    );
    assert_eq!(rep.issues[0].target, v(&f.dst));
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

#[cfg(unix)]
mod unix {
    use super::*;
    use rf_jobs::SymlinkMode;
    use rf_jobs::testkit::{Node, ResolverCall};
    use std::os::unix::fs::symlink;

    fn follow() -> TransferOptions {
        TransferOptions {
            symlinks: SymlinkMode::Follow,
            ..TransferOptions::default()
        }
    }

    #[test]
    fn symlinks_are_copied_as_links() {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A")]);
        symlink("a.txt", f.src.join("dir").join("link")).unwrap();
        assert_eq!(
            run(&copy(&[f.src.join("dir")], &f.dst)).outcome,
            Outcome::Completed
        );
        assert_eq!(tree(&f.dst.join("dir"))["link"], Node::Link("a.txt".into()));
    }

    #[test]
    fn follow_mode_copies_what_links_point_to() {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A"), ("outside/b.txt", "B")]);
        symlink("a.txt", f.src.join("dir").join("flink")).unwrap();
        symlink(f.src.join("outside"), f.src.join("dir").join("dlink")).unwrap();
        assert_eq!(
            run(&spec(
                TransferMode::Copy,
                &[f.src.join("dir")],
                &f.dst,
                follow()
            ))
            .outcome,
            Outcome::Completed
        );
        let t = tree(&f.dst.join("dir"));
        assert_eq!(
            (&t["flink"], &t["dlink/b.txt"]),
            (&Node::File(b"A".to_vec()), &Node::File(b"B".to_vec()))
        );
    }

    #[test]
    fn follow_mode_stops_link_loops() {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A")]);
        symlink(".", f.src.join("dir").join("loop")).unwrap();
        let rep = run(&spec(
            TransferMode::Copy,
            &[f.src.join("dir")],
            &f.dst,
            follow(),
        ));
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        // Controller ruling (Task 10 fix round 1): a followed folder that is a folder above it
        // stops as a loop of links; the depth limit is the fallback without identities.
        assert!(
            rep.failures.iter().any(|x| x.message.contains("link loop")),
            "{:?}",
            rep.failures
        );
    }

    /// Without identities a loop cannot be recognised, and the depth limit ends it.
    #[test]
    fn follow_mode_ends_link_loops_at_the_depth_limit_without_identities() {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A")]);
        symlink(".", f.src.join("dir").join("loop")).unwrap();
        let p = FaultyProvider::local(vec![Fault::NoIdentity]);
        let job = spec(TransferMode::Copy, &[f.src.join("dir")], &f.dst, follow());
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert!(
            rep.failures
                .iter()
                .all(|x| x.message.contains("nesting deeper than")),
            "{:?}",
            rep.failures
        );
        // `a.txt` and `loop` one level below the limit.
        assert_eq!(rep.failures.len(), 2);
        assert_eq!(rep.planned, rep.done);
    }

    /// Two links back into one folder would double the walk at each level; each is stopped
    /// where it first leads back, so the job ends at once. So does a pair of folders that
    /// lead into each other.
    #[test]
    fn follow_mode_stops_every_link_back_into_a_folder_above() {
        let f = fx();
        write_tree(
            &f.src,
            &[("dir/a.txt", "A"), ("dir/x/", ""), ("dir/y/", "")],
        );
        let dir = f.src.join("dir");
        symlink(".", dir.join("l1")).unwrap();
        symlink(".", dir.join("l2")).unwrap();
        symlink("../y", dir.join("x").join("lx")).unwrap();
        symlink("../x", dir.join("y").join("ly")).unwrap();
        let started = std::time::Instant::now();
        let rep = run(&spec(
            TransferMode::Copy,
            std::slice::from_ref(&dir),
            &f.dst,
            follow(),
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        let looped: Vec<_> = rep
            .failures
            .iter()
            .map(|x| (x.stage, x.kind, x.message.contains("link loop")))
            .collect();
        assert_eq!(
            looped,
            [(Stage::Plan, ErrorKind::Unsupported, true); 4],
            "{:?}",
            rep.failures
        );
        // Below a followed link the sources are resolved paths (`/private/var` on macOS), so
        // only the names of the links are compared.
        let mut links: Vec<_> = rep
            .failures
            .iter()
            .filter_map(|x| x.source.as_ref()?.file_name().map(ToOwned::to_owned))
            .collect();
        links.sort();
        assert_eq!(links, ["l1", "l2", "lx", "ly"]);
        assert_eq!(
            tree(&f.dst.join("dir")).into_keys().collect::<Vec<_>>(),
            ["a.txt", "x", "x/lx", "y", "y/ly"]
        );
        assert_eq!(rep.planned, rep.done);
    }

    /// The targets the review left out stay out under a folder written under a numbered name.
    #[test]
    fn skipped_targets_stay_skipped_under_a_renamed_folder() {
        let f = fx();
        write_tree(&f.src, &[("dir/a?b.txt", "x"), ("dir/ok.txt", "y")]);
        write_tree(&f.dst, &[("dir", "a file")]);
        let p = FaultyProvider::local(vec![Fault::FsName("exFAT".into())]);
        let r = ScriptedResolver::new().on_preflight(PreflightAnswer::SkipBlocked);
        let job = spec(
            TransferMode::Copy,
            &[f.src.join("dir")],
            &f.dst,
            with(ConflictPolicy::AutoRename),
        );
        let rep = execute(
            JobId::generate(),
            &job,
            &ExecEnv::new(&p.vfs(), &r).naming_os(Os::Windows),
        );
        let renamed = f.dst.join("dir (2)");
        let failed: Vec<_> = rep
            .failures
            .iter()
            .map(|x| (x.stage, x.target.clone()))
            .collect();
        assert_eq!(
            failed,
            [(Stage::Preflight, Some(v(renamed.join("a?b.txt"))))]
        );
        assert_eq!(tree(&renamed).into_keys().collect::<Vec<_>>(), ["ok.txt"]);
        assert_eq!(
            std::fs::read_to_string(f.dst.join("dir")).unwrap(),
            "a file"
        );
        assert_eq!(r.calls(), [ResolverCall::Preflight { issues: 1 }]);
    }

    #[test]
    fn skip_blocked_skips_only_names_the_target_cannot_hold() {
        let f = fx();
        write_tree(&f.src, &[("a?b.txt", "x"), ("ok.txt", "y")]);
        let p = FaultyProvider::local(vec![Fault::FsName("exFAT".into())]);
        let r = ScriptedResolver::new().on_preflight(PreflightAnswer::SkipBlocked);
        let rep = run_on(
            &copy(&[f.src.join("a?b.txt"), f.src.join("ok.txt")], &f.dst),
            &p.vfs(),
            &r,
        );
        assert_eq!(
            rep.issues
                .iter()
                .map(|i| i.kind.clone())
                .collect::<Vec<_>>(),
            [IssueKind::InvalidCharacter('?')]
        );
        assert_eq!(
            (rep.failures.len(), rep.failures[0].stage),
            (1, Stage::Preflight)
        );
        assert_eq!(tree(&f.dst).into_keys().collect::<Vec<_>>(), ["ok.txt"]);
        assert_eq!(r.calls(), [ResolverCall::Preflight { issues: 1 }]);
    }

    #[test]
    fn preflight_cancel_writes_nothing() {
        let f = fx();
        write_tree(&f.src, &[("a?b.txt", "x"), ("ok.txt", "y")]);
        let p = FaultyProvider::local(vec![Fault::FsName("exFAT".into())]);
        let r = ScriptedResolver::new().on_preflight(PreflightAnswer::Cancel);
        let rep = run_on(
            &copy(&[f.src.join("a?b.txt"), f.src.join("ok.txt")], &f.dst),
            &p.vfs(),
            &r,
        );
        assert_eq!(rep.outcome, Outcome::Cancelled);
        assert_eq!(tree(&f.dst), BTreeMap::new());
    }

    #[test]
    fn special_files_are_reported_not_opened() {
        use std::os::unix::net::UnixListener;
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A")]);
        let (top, inner) = (f.src.join("top.sock"), f.src.join("dir").join("inner.sock"));
        let _sockets = (
            UnixListener::bind(&top).unwrap(),
            UnixListener::bind(&inner).unwrap(),
        );
        let rep = run(&copy(&[top.clone(), f.src.join("dir")], &f.dst));
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        let failed: Vec<_> = rep
            .failures
            .iter()
            .map(|x| (x.stage, x.kind, x.source.clone()))
            .collect();
        assert_eq!(
            failed,
            [
                (Stage::Plan, ErrorKind::Unsupported, Some(v(&top))),
                (Stage::Plan, ErrorKind::Unsupported, Some(v(&inner))),
            ]
        );
        assert_eq!(
            tree(&f.dst).into_keys().collect::<Vec<_>>(),
            ["dir", "dir/a.txt"]
        );
    }

    // What follows goes beyond the brief: the rules the controller added for this task.

    /// A link to a folder at the target is no folder: nothing is merged into it, so nothing is
    /// written through it.
    #[test]
    fn a_link_to_a_folder_at_the_target_is_never_merged_into() {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "new")]);
        write_tree(&f.root, &[("outside/a.txt", "theirs")]);
        let outside = f.root.join("outside");
        symlink(&outside, f.dst.join("dir")).unwrap();
        let theirs = BTreeMap::from([("a.txt".to_string(), Node::File(b"theirs".to_vec()))]);

        let rep = run(&spec(
            TransferMode::Copy,
            &[f.src.join("dir")],
            &f.dst,
            with(ConflictPolicy::Overwrite),
        ));
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            (rep.failures[0].stage, rep.failures[0].kind),
            (Stage::Conflict, ErrorKind::AlreadyExists)
        );
        assert_eq!(tree(&outside), theirs);
        assert_eq!(
            tree(&f.dst),
            BTreeMap::from([("dir".to_string(), Node::Link(outside.clone()))])
        );

        // Keeping both makes a real folder next to the link.
        let job = spec(
            TransferMode::Copy,
            &[f.src.join("dir")],
            &f.dst,
            with(ConflictPolicy::AutoRename),
        );
        let rep = run_with(&job, &Vfs::with_local(), Os::Windows);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!(rep.outputs, [v(f.dst.join("dir (2)"))]);
        assert_eq!(
            std::fs::read_to_string(f.dst.join("dir (2)").join("a.txt")).unwrap(),
            "new"
        );
        assert_eq!(tree(&outside), theirs);
    }

    /// A file or a link copied over a link at the target replaces the link itself; what the
    /// link leads to stays as it is.
    #[test]
    fn overwriting_a_link_replaces_the_link_itself() {
        let f = fx();
        write_tree(&f.src, &[("a.txt", "new"), ("b.txt", "B")]);
        symlink("b.txt", f.src.join("link")).unwrap();
        write_tree(&f.root, &[("outside.txt", "theirs")]);
        let outside = f.root.join("outside.txt");
        symlink(&outside, f.dst.join("a.txt")).unwrap();
        symlink(&outside, f.dst.join("link")).unwrap();
        let rep = run(&spec(
            TransferMode::Copy,
            &[f.src.join("a.txt"), f.src.join("link")],
            &f.dst,
            with(ConflictPolicy::Overwrite),
        ));
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "theirs");
        assert_eq!(
            tree(&f.dst),
            BTreeMap::from([
                ("a.txt".to_string(), Node::File(b"new".to_vec())),
                ("link".to_string(), Node::Link("b.txt".into())),
            ])
        );
    }

    /// The mode bits of a read-only folder are put on its copy after the children are written,
    /// so a user who is not root can still write them.
    #[test]
    fn a_read_only_folder_gets_its_mode_after_its_children() {
        use std::os::unix::fs::PermissionsExt;
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
        let (dir, copied) = (f.src.join("dir"), f.dst.join("dir"));
        let set_mode = |p: &std::path::Path, mode| {
            std::fs::set_permissions(p, PermissionsExt::from_mode(mode))
        };
        set_mode(&dir, 0o555).unwrap();
        let rep = run(&copy(std::slice::from_ref(&dir), &f.dst));
        // Writable again before anything can fail, so the temporary folder can be removed.
        set_mode(&dir, 0o755).unwrap();
        let mode = std::fs::metadata(&copied).map(|m| m.permissions().mode() & 0o777);
        let _ = set_mode(&copied, 0o755);
        assert_eq!(
            (rep.outcome, rep.warnings.len()),
            (Outcome::Completed, 0),
            "{:?} {:?}",
            rep.failures,
            rep.warnings
        );
        assert_eq!(mode.unwrap(), 0o555);
        assert_eq!(tree(&copied), tree(&dir));
    }

    /// In Follow mode a copy gets the extras (here the mode bits) of the item a link leads to,
    /// found the way the OS finds it: a `..` in a link goes up from the folder the link really
    /// is in, also when that folder was reached through a link.
    #[test]
    fn follow_mode_copies_take_the_mode_of_what_links_point_to() {
        use std::os::unix::fs::PermissionsExt;
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("dir/run.sh", "decoy"),
                ("outside/run.sh", "real"),
                ("outside/sub/", ""),
            ],
        );
        let set_mode = |p: std::path::PathBuf, mode| {
            std::fs::set_permissions(p, PermissionsExt::from_mode(mode)).unwrap();
        };
        set_mode(f.src.join("dir").join("run.sh"), 0o600);
        set_mode(f.src.join("outside").join("run.sh"), 0o751);
        symlink("../outside/run.sh", f.src.join("dir").join("flink")).unwrap();
        symlink("../outside/sub", f.src.join("dir").join("dlink")).unwrap();
        // Read as text through `dir/dlink`, this leads to the decoy `dir/run.sh`.
        symlink("../run.sh", f.src.join("outside").join("sub").join("x")).unwrap();
        let rep = run(&spec(
            TransferMode::Copy,
            &[f.src.join("dir")],
            &f.dst,
            follow(),
        ));
        assert_eq!(
            (rep.outcome, rep.warnings.len()),
            (Outcome::Completed, 0),
            "{:?} {:?}",
            rep.failures,
            rep.warnings
        );
        let copied = f.dst.join("dir");
        let mode = |p: std::path::PathBuf| {
            let meta = std::fs::metadata(p).unwrap();
            (meta.permissions().mode() & 0o777, meta.len())
        };
        assert_eq!(mode(copied.join("flink")), (0o751, 4));
        assert_eq!(mode(copied.join("dlink").join("x")), (0o751, 4));
        assert_eq!(mode(copied.join("run.sh")), (0o600, 5));
    }

    /// A followed link that leads to a FIFO, socket or device is reported and never opened.
    #[test]
    fn follow_mode_reports_links_to_special_files() {
        use std::os::unix::net::UnixListener;
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A")]);
        let socket = f.src.join("s.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        symlink(&socket, f.src.join("dir").join("slink")).unwrap();
        let rep = run(&spec(
            TransferMode::Copy,
            &[f.src.join("dir")],
            &f.dst,
            follow(),
        ));
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        let failed: Vec<_> = rep.failures.iter().map(|x| (x.stage, x.kind)).collect();
        assert_eq!(failed, [(Stage::Plan, ErrorKind::Unsupported)]);
        assert_eq!(
            tree(&f.dst).into_keys().collect::<Vec<_>>(),
            ["dir", "dir/a.txt"]
        );
    }

    /// The failure of an item the review left out carries the issue's own log text.
    #[test]
    fn a_blocked_item_fails_with_the_issue_as_its_message() {
        let f = fx();
        write_tree(&f.src, &[("a?b.txt", "x")]);
        let p = FaultyProvider::local(vec![Fault::FsName("exFAT".into())]);
        let rep = run_on(
            &copy(&[f.src.join("a?b.txt")], &f.dst),
            &p.vfs(),
            &NonInteractive,
        );
        assert_eq!(
            rep.failures,
            [rf_jobs::ItemFailure {
                source: Some(v(f.src.join("a?b.txt"))),
                target: Some(v(f.dst.join("a?b.txt"))),
                stage: Stage::Preflight,
                kind: ErrorKind::InvalidPath,
                message: IssueKind::InvalidCharacter('?').to_string(),
            }]
        );
        assert_eq!(tree(&f.dst), BTreeMap::new());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn case_collisions_are_reported_for_case_insensitive_targets() {
    use rf_jobs::testkit::ResolverCall;
    let f = fx();
    write_tree(&f.src, &[("dir/Readme", "1"), ("dir/README", "2")]);
    let p = FaultyProvider::local(vec![Fault::FsName("NTFS".into())]);
    let r = ScriptedResolver::new().on_preflight(PreflightAnswer::Proceed);
    let rep = run_on(&copy(&[f.src.join("dir")], &f.dst), &p.vfs(), &r);
    assert!(
        rep.issues
            .iter()
            .any(|i| matches!(i.kind, IssueKind::CaseCollision { .. })),
        "{:?}",
        rep.issues
    );
    assert_eq!(r.calls(), [ResolverCall::Preflight { issues: 1 }]);
}

#[cfg(windows)]
#[test]
fn windows_symlinks_are_copied_as_links() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    if let Err(e) = std::os::windows::fs::symlink_file("a.txt", f.src.join("dir").join("link")) {
        eprintln!("skipped: cannot create symlinks here: {e}");
        return;
    }
    assert_eq!(
        run(&copy(&[f.src.join("dir")], &f.dst)).outcome,
        Outcome::Completed
    );
    assert_eq!(
        std::fs::read_link(f.dst.join("dir").join("link")).unwrap(),
        std::path::PathBuf::from("a.txt")
    );
}

/// `C:\Work\A` -> `C:\work\a\sub`: NTFS sees one folder where `VPath::starts_with` sees two.
#[cfg(windows)]
#[test]
fn copying_a_folder_into_itself_ignores_case_on_windows() {
    let f = fx();
    write_tree(&f.src, &[("Work/A/sub/", "")]);
    let rep = run(&copy(
        &[f.src.join("Work").join("A")],
        &f.src.join("work").join("a").join("sub"),
    ));
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Plan, ErrorKind::Unsupported)
    );
    assert!(
        rep.failures[0].message.contains("into itself"),
        "{:?}",
        rep.failures
    );
    assert_eq!(
        tree(&f.src.join("Work").join("A").join("sub")),
        BTreeMap::new()
    );
}

// What follows goes beyond the brief: the rules the controller added for this task.

fn with(policy: ConflictPolicy) -> TransferOptions {
    TransferOptions {
        conflict: policy,
        ..TransferOptions::default()
    }
}

fn run_with(job: &JobSpec, vfs: &Vfs, os: Os) -> rf_jobs::JobReport {
    execute(
        JobId::generate(),
        job,
        &ExecEnv::new(vfs, &NonInteractive).naming_os(os),
    )
}

/// DND-11: a folder pasted into the folder it is in gets a numbered copy, whatever the
/// policy; it is never merged into itself.
#[test]
fn copying_a_folder_onto_itself_makes_a_numbered_copy() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
    let before = tree(&f.src.join("dir"));
    let job = spec(
        TransferMode::Copy,
        &[f.src.join("dir")],
        &f.src,
        with(ConflictPolicy::Overwrite),
    );
    let rep = run_with(&job, &Vfs::with_local(), Os::Windows);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(rep.outputs, [v(f.src.join("dir (2)"))]);
    assert_eq!(tree(&f.src.join("dir")), before);
    assert_eq!(tree(&f.src.join("dir (2)")), before);
    assert_eq!(rep.planned, rep.done);
}

/// A folder and a file never replace each other, whatever the policy.
#[test]
fn a_folder_never_replaces_a_file() {
    let f = fx();
    write_tree(&f.src, &[("x/a.txt", "A")]);
    write_tree(&f.dst, &[("x", "keep")]);
    let rep = run(&spec(
        TransferMode::Copy,
        &[f.src.join("x")],
        &f.dst,
        with(ConflictPolicy::Overwrite),
    ));
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Conflict, ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read_to_string(f.dst.join("x")).unwrap(), "keep");
}

/// On a case-insensitive target whose file system gives no identities, a target spelled like
/// the source but for case is the source: the file gets a numbered copy instead of replacing
/// it, and the folder one instead of a merge into itself.
#[cfg(target_os = "linux")]
#[test]
fn a_case_only_spelling_of_the_source_is_the_source() {
    use rf_jobs::{TransferItem, TransferSpec};
    let f = fx();
    // ext4 keeps both spellings apart; the fake NTFS does not.
    write_tree(
        &f.src,
        &[
            ("a.txt", "me"),
            ("A.TXT", "other"),
            ("dir/b.txt", "B"),
            ("DIR/c.txt", "C"),
        ],
    );
    let p = FaultyProvider::local(vec![Fault::FsName("NTFS".into()), Fault::NoIdentity]);
    let item = |from: &str, to: &str| TransferItem {
        source: v(f.src.join(from)),
        target: v(f.src.join(to)),
    };
    let job = JobSpec::Transfer(TransferSpec {
        mode: TransferMode::Copy,
        items: vec![item("a.txt", "A.TXT"), item("dir", "DIR")],
        options: with(ConflictPolicy::Overwrite),
    });
    let rep = run_with(&job, &p.vfs(), Os::Windows);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(
        rep.outputs,
        [v(f.src.join("A (2).TXT")), v(f.src.join("DIR (2)"))]
    );
    assert_eq!(
        std::fs::read_to_string(f.src.join("A.TXT")).unwrap(),
        "other"
    );
    assert_eq!(
        tree(&f.src.join("DIR")).into_keys().collect::<Vec<_>>(),
        ["c.txt"]
    );
    assert_eq!(
        tree(&f.src.join("DIR (2)")).into_keys().collect::<Vec<_>>(),
        ["b.txt"]
    );
}

/// The Linux twin of `copying_a_folder_into_itself_ignores_case_on_windows`, on a fake NTFS.
#[cfg(target_os = "linux")]
#[test]
fn copying_a_folder_into_itself_ignores_case_on_case_insensitive_targets() {
    let f = fx();
    write_tree(&f.src, &[("Work/A/sub/", ""), ("work/a/sub/", "")]);
    let p = FaultyProvider::local(vec![Fault::FsName("NTFS".into())]);
    let rep = run_on(
        &copy(
            &[f.src.join("Work").join("A")],
            &f.src.join("work").join("a").join("sub"),
        ),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Plan, ErrorKind::Unsupported)
    );
    assert!(
        rep.failures[0].message.contains("into itself"),
        "{:?}",
        rep.failures
    );
    assert_eq!(
        tree(&f.src.join("work")).into_keys().collect::<Vec<_>>(),
        ["a", "a/sub"]
    );
    // Only the case differs: a copy of `Work/A` named `work/A` is no copy into itself.
    let rep = run_on(
        &copy(&[f.src.join("Work").join("A")], &f.src.join("work")),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
}

/// The outputs are the top-level items the job made: a folder it merged into was there
/// before.
#[test]
fn a_merged_folder_is_not_an_output() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("new/b.txt", "B")]);
    write_tree(&f.dst, &[("dir/other.txt", "O")]);
    let rep = run(&copy(&[f.src.join("dir"), f.src.join("new")], &f.dst));
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(rep.outputs, [v(f.dst.join("new"))]);
}

/// A folder's own step that fails and is tried again leaves its children counted once.
#[test]
fn retrying_a_folder_counts_its_children_once() {
    use rf_jobs::testkit::ResolverCall;
    use rf_jobs::{ErrorAction, ErrorAnswer, ErrorPolicy, Totals};
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
    // The first try of `dir` fails, and so does the first try of `sub`.
    let p = FaultyProvider::local(vec![
        Fault::FailCreateDir { call: 0 },
        Fault::FailCreateDir { call: 2 },
    ]);
    let retry = ErrorAnswer {
        action: ErrorAction::Retry,
        apply_to_all: false,
    };
    let r = ScriptedResolver::new().on_error(retry).on_error(retry);
    let options = TransferOptions {
        errors: ErrorPolicy::Ask,
        ..TransferOptions::default()
    };
    let rep = run_on(
        &spec(TransferMode::Copy, &[f.src.join("dir")], &f.dst, options),
        &p.vfs(),
        &r,
    );
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    let write_failed = ResolverCall::Error {
        stage: Stage::Write,
    };
    assert_eq!(r.calls(), [write_failed.clone(), write_failed]);
    let all = Totals {
        files: 2,
        dirs: 2,
        bytes: 2,
    };
    assert_eq!((rep.planned, rep.done), (all, all));
    assert_eq!(tree(&f.dst.join("dir")), tree(&f.src.join("dir")));
}
