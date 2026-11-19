//! Folder listing benchmarks (PERF-03, PERF-04).
//!
//! `cargo bench -p rf-vfs --bench list` measures the provider's share of the budget in the
//! Global Constraints; `-- --test` runs each benchmark once as a smoke test. The fixtures are
//! folders of empty files in the system temporary folder; they are built on the first run and
//! reused afterwards.

use std::fs::File;
use std::hint::black_box;
use std::io::ErrorKind;
use std::path::PathBuf;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use rf_core::identity::TEMP_PREFIX;
use rf_core::{Entry, Error, VPath};
use rf_vfs::{CancelToken, LocalProvider, Provider, SortSpec, sort_entries};

/// The folder and its marker file live in the system temporary folder under this name.
fn fixture_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{TEMP_PREFIX}bench-{name}"))
}

/// A folder of `files` empty files and `dirs` subfolders, built once and then reused.
///
/// A marker file next to the folder holds `"{files}+{dirs}"` and is written last, so a folder
/// whose marker is missing or different (an interrupted or older build) is deleted and built
/// again. Every tenth file is a Korean-named `.jpg`; the others are `file_NNNNNN.txt`.
fn fixture(name: &str, files: usize, dirs: usize) -> PathBuf {
    let dir = fixture_path(name);
    let marker = dir.with_file_name(format!("{TEMP_PREFIX}bench-{name}.complete"));
    let wanted = format!("{files}+{dirs}");
    if std::fs::read_to_string(&marker).is_ok_and(|text| text == wanted) {
        return dir;
    }
    if let Err(e) = std::fs::remove_file(&marker) {
        assert_eq!(e.kind(), ErrorKind::NotFound, "cannot remove the marker");
    }
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert_eq!(e.kind(), ErrorKind::NotFound, "cannot remove the folder");
    }
    std::fs::create_dir_all(&dir).expect("create the fixture folder");
    for i in 0..files {
        let file_name = if i % 10 == 0 {
            format!("사진 {i:06}.jpg")
        } else {
            format!("file_{i:06}.txt")
        };
        File::create(dir.join(file_name)).expect("create a fixture file");
    }
    for i in 0..dirs {
        std::fs::create_dir(dir.join(format!("dir_{i:05}"))).expect("create a fixture subfolder");
    }
    std::fs::write(&marker, wanted).expect("write the marker");
    dir
}

fn vpath(dir: &std::path::Path) -> VPath {
    VPath::local(dir).expect("the temporary folder is an absolute local path")
}

/// Lists `dir` completely and returns how many entries came back; the batches are dropped as
/// they arrive.
fn count_entries(provider: LocalProvider, dir: &VPath) -> usize {
    let mut count = 0;
    provider
        .list(dir, &CancelToken::new(), &mut |batch| count += batch.len())
        .expect("list the fixture");
    count
}

fn list_benches(c: &mut Criterion) {
    let provider = LocalProvider::new();
    let small = vpath(&fixture("1k", 950, 50));
    let large = vpath(&fixture("100k", 95_000, 5_000));

    let mut group = c.benchmark_group("list");
    group.bench_function("1k_full", |b| {
        b.iter(|| black_box(count_entries(provider, &small)));
    });

    group.sample_size(10);
    group.bench_function("100k_first_batch", |b| {
        b.iter(|| {
            // Time to the first screenful: cancel as soon as the first batch arrives.
            let cancel = CancelToken::new();
            let result = provider.list(&large, &cancel, &mut |batch| {
                black_box(batch.len());
                cancel.cancel();
            });
            assert!(matches!(result, Err(Error::Cancelled)));
        });
    });
    group.bench_function("100k_full", |b| {
        b.iter(|| black_box(count_entries(provider, &large)));
    });
    group.finish();
}

fn sort_benches(c: &mut Criterion) {
    let provider = LocalProvider::new();
    let large = vpath(&fixture("100k", 95_000, 5_000));
    let mut entries: Vec<Entry> = Vec::new();
    provider
        .list(&large, &CancelToken::new(), &mut |batch| {
            entries.extend(batch);
        })
        .expect("list the fixture");
    let spec = SortSpec::default();

    let mut group = c.benchmark_group("sort");
    group.sample_size(10);
    group.bench_function("100k_name", |b| {
        b.iter_batched(
            || entries.clone(),
            |mut v| sort_entries(&mut v, &spec),
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

/// The floor for the listing numbers: the operating system's directory read and nothing else.
fn raw_benches(c: &mut Criterion) {
    let large = fixture("100k", 95_000, 5_000);

    let mut group = c.benchmark_group("raw");
    group.sample_size(10);
    group.bench_function("100k_read_dir", |b| {
        b.iter(|| {
            let mut count = 0_usize;
            for item in std::fs::read_dir(&large).expect("read the fixture") {
                black_box(item.expect("read an item").file_name());
                count += 1;
            }
            black_box(count)
        });
    });
    group.finish();
}

criterion_group!(benches, list_benches, sort_benches, raw_benches);
criterion_main!(benches);
