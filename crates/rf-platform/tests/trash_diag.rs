//! Temporary diagnostics for the windows-11-arm recycle failure (diag branch only).
#![cfg(windows)]
#![allow(clippy::all, clippy::pedantic)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rf_core::VPath;
use rf_platform::move_to_trash;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn unique(dir: &Path, suffix: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("RacoonFinder-diag-{}-{n}{suffix}", std::process::id()))
}

fn purge(location: &Path) {
    if location.is_dir() {
        let _ = std::fs::remove_dir_all(location);
    } else {
        let _ = std::fs::remove_file(location);
    }
    let name = location.file_name().unwrap().to_string_lossy().into_owned();
    if let Some(rest) = name.strip_prefix("$R") {
        let _ = std::fs::remove_file(location.with_file_name(format!("$I{rest}")));
    }
}

fn make_link(root: &Path) -> (PathBuf, PathBuf) {
    let target = unique(root, "-target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep.txt"), b"keep").unwrap();
    let link = unique(root, "-link");
    std::os::windows::fs::symlink_dir(&target, &link).unwrap();
    (target, link)
}

fn make_file(root: &Path) -> PathBuf {
    let f = unique(root, ".txt");
    std::fs::write(&f, b"x").unwrap();
    f
}

fn make_long(root: &Path) -> PathBuf {
    let mut dir = root.to_path_buf();
    dir.push(format!("L{}", SEQ.fetch_add(1, Ordering::Relaxed)));
    for i in 0..4 {
        dir.push(format!("{i}-{}", "x".repeat(80)));
    }
    std::fs::create_dir_all(&dir).unwrap();
    let f = unique(&dir, ".txt");
    std::fs::write(&f, b"keep").unwrap();
    f
}

fn trash(label: &str, p: &Path, do_purge: bool) -> bool {
    let start = Instant::now();
    let r = move_to_trash(&VPath::local(p).unwrap());
    eprintln!(
        "RESULT {label}: {:?} in {:?} ({})",
        r,
        start.elapsed(),
        p.display()
    );
    let ok = matches!(r, Ok(Some(_)));
    if do_purge {
        if let Ok(Some(loc)) = &r {
            purge(loc);
        }
    }
    ok
}

/// One test so that the scenarios run in a fixed order, one after another.
#[test]
fn scenarios() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    eprintln!("== bitbucket dir before: {:?}", std::fs::read_dir("C:\\$Recycle.Bin").map(|r| r.count()));

    eprintln!("== S1 serial dir symlinks x5");
    let mut s1 = 0;
    for _ in 0..5 {
        let (_t, l) = make_link(root);
        s1 += usize::from(trash("S1 link", &l, true));
    }

    eprintln!("== S2 file then link x3");
    let mut s2 = 0;
    for _ in 0..3 {
        trash("S2 file", &make_file(root), true);
        let (_t, l) = make_link(root);
        s2 += usize::from(trash("S2 link", &l, true));
    }

    eprintln!("== S3 long path then link x3");
    let mut s3 = 0;
    for _ in 0..3 {
        trash("S3 long", &make_long(root), true);
        let (_t, l) = make_link(root);
        s3 += usize::from(trash("S3 link", &l, true));
    }

    eprintln!("== S4 link aged 3 s x2");
    let mut s4 = 0;
    for _ in 0..2 {
        let (_t, l) = make_link(root);
        std::thread::sleep(Duration::from_secs(3));
        s4 += usize::from(trash("S4 link aged", &l, true));
    }

    eprintln!("== S5 concurrent: file, dir, long, link at once x4");
    let mut s5 = 0;
    for round in 0..4 {
        let f = make_file(root);
        let d = unique(root, "-dir");
        std::fs::create_dir_all(d.join("inner")).unwrap();
        std::fs::write(d.join("inner").join("f.txt"), b"x").unwrap();
        let lp = make_long(root);
        let (_t, l) = make_link(root);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut hs = Vec::new();
        for (label, p) in [("S5 file", f), ("S5 dir", d), ("S5 long", lp), ("S5 link", l)] {
            let b = barrier.clone();
            hs.push(std::thread::spawn(move || {
                b.wait();
                (label, trash(&format!("{label} r{round}"), &p, true))
            }));
        }
        for h in hs {
            let (label, ok) = h.join().unwrap();
            if label == "S5 link" {
                s5 += usize::from(ok);
            }
        }
    }

    eprintln!("== S6 concurrent links only x3 rounds of 4");
    let mut s6 = 0;
    for round in 0..3 {
        let links: Vec<_> = (0..4).map(|_| make_link(root).1).collect();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let hs: Vec<_> = links
            .into_iter()
            .map(|l| {
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    trash(&format!("S6 link r{round}"), &l, true)
                })
            })
            .collect();
        s6 += hs.into_iter().map(|h| usize::from(h.join().unwrap())).sum::<usize>();
    }

    eprintln!("== S7 concurrent links without purge, then purge x2 rounds of 3");
    let mut s7 = 0;
    for round in 0..2 {
        let links: Vec<_> = (0..3).map(|_| make_link(root).1).collect();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let hs: Vec<_> = links
            .into_iter()
            .map(|l| {
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    trash(&format!("S7 link r{round}"), &l, false)
                })
            })
            .collect();
        s7 += hs.into_iter().map(|h| usize::from(h.join().unwrap())).sum::<usize>();
    }

    eprintln!(
        "SUMMARY S1 {s1}/5 S2 {s2}/3 S3 {s3}/3 S4 {s4}/2 S5 {s5}/4 S6 {s6}/12 S7 {s7}/6"
    );
}
