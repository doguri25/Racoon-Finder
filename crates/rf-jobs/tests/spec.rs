#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

use std::collections::BTreeSet;
use std::ffi::OsString;

use rf_core::{ErrorKind, Scheme, VPath};
use rf_jobs::{
    ConflictPolicy, CreateKind, DeleteMethod, DeleteSpec, ErrorPolicy, JobId, JobKind, JobSpec,
    SymlinkMode, TransferItem, TransferMode, TransferOptions, TransferSpec,
};

fn root() -> VPath {
    VPath::local(if cfg!(windows) { r"C:\" } else { "/" }).unwrap()
}

fn at(parts: &[&str]) -> VPath {
    parts.iter().fold(root(), |p, n| p.join(n).unwrap())
}

#[test]
fn job_ids_are_unique_and_hex() {
    let ids: BTreeSet<JobId> = (0..1_000).map(|_| JobId::generate()).collect();
    assert_eq!(ids.len(), 1_000);
    assert_eq!(JobId(0xab).to_string(), "00000000000000ab");
    assert_eq!(JobId::from_hex("00000000000000ab"), Some(JobId(0xab)));
    assert_eq!(JobId::from_hex("ab"), None);
    assert_eq!(JobId::from_hex("+0000000000000ab"), None);
    assert_eq!(JobId::from_hex("zz000000000000ab"), None);
}

#[test]
fn defaults_follow_the_prd() {
    let o = TransferOptions::default();
    assert_eq!(
        (o.conflict, o.errors, o.symlinks),
        (
            ConflictPolicy::Ask,
            ErrorPolicy::Continue,
            SymlinkMode::CopyLink
        )
    );
}

#[test]
fn into_dir_targets_each_source_by_name() {
    let sources = [at(&["a", "x.txt"]), at(&["b", "dir"])];
    let spec = TransferSpec::into_dir(
        TransferMode::Move,
        &sources,
        &at(&["dst"]),
        TransferOptions::default(),
    )
    .unwrap();
    assert_eq!(
        spec.items,
        [
            TransferItem {
                source: at(&["a", "x.txt"]),
                target: at(&["dst", "x.txt"])
            },
            TransferItem {
                source: at(&["b", "dir"]),
                target: at(&["dst", "dir"])
            },
        ]
    );
    assert_eq!(JobSpec::Transfer(spec).kind(), JobKind::Move);
}

#[test]
fn into_dir_refuses_volume_roots() {
    let err = TransferSpec::into_dir(
        TransferMode::Copy,
        &[root()],
        &at(&["dst"]),
        TransferOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported);
}

#[test]
fn kinds_and_journaling() {
    let del = JobSpec::Delete(DeleteSpec {
        paths: vec![at(&["x"])],
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    });
    let ren = JobSpec::Rename {
        from: at(&["a"]),
        to: at(&["b"]),
    };
    let mk = JobSpec::Create {
        parent: root(),
        name: OsString::from("새 폴더"),
        kind: CreateKind::Dir,
    };
    let copy = JobSpec::Transfer(TransferSpec {
        mode: TransferMode::Copy,
        items: vec![],
        options: TransferOptions::default(),
    });
    assert_eq!(
        [del.kind(), ren.kind(), mk.kind(), copy.kind()],
        [
            JobKind::Delete,
            JobKind::Rename,
            JobKind::Create,
            JobKind::Copy
        ]
    );
    assert_eq!(
        [
            del.journaled(),
            ren.journaled(),
            mk.journaled(),
            copy.journaled()
        ],
        [true, false, false, true]
    );
}

#[test]
fn specs_roundtrip_through_json() {
    let opts = TransferOptions {
        conflict: ConflictPolicy::OverwriteIfNewer,
        errors: ErrorPolicy::Ask,
        symlinks: SymlinkMode::Follow,
    };
    let specs = [
        JobSpec::Transfer(
            TransferSpec::into_dir(
                TransferMode::Copy,
                &[at(&["한글 100%.txt"])],
                &at(&["dst"]),
                opts,
            )
            .unwrap(),
        ),
        JobSpec::Delete(DeleteSpec {
            paths: vec![VPath::new(Scheme::Remote, "p", &["x"]).unwrap()],
            method: DeleteMethod::Permanent,
            errors: ErrorPolicy::Abort,
        }),
        JobSpec::Create {
            parent: root(),
            name: OsString::from("🦝"),
            kind: CreateKind::File,
        },
    ];
    for spec in specs {
        let json = serde_json::to_string(&spec).unwrap();
        assert_eq!(
            serde_json::from_str::<JobSpec>(&json).unwrap(),
            spec,
            "{json}"
        );
    }
}
