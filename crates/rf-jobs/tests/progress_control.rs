#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use rf_core::{Entry, EntryKind, ErrorKind, FileId, VPath, VolumeId};
use rf_jobs::{
    Control, CreatedItem, ItemFailure, MovedItem, SpeedMeter, Stage, TrashedItem, UndoRecord,
};

#[test]
fn speed_meter_averages_over_the_window() {
    let t0 = Instant::now();
    let s = |secs| t0 + Duration::from_secs(secs);
    let mut m = SpeedMeter::new(Duration::from_secs(3));
    assert_eq!(m.bytes_per_sec(), 0);
    m.record(s(0), 0);
    assert_eq!(m.bytes_per_sec(), 0);
    m.record(s(1), 100);
    m.record(s(2), 300);
    assert_eq!(m.bytes_per_sec(), 150);
    assert_eq!(m.eta(300), Some(Duration::from_secs(2)));
    m.record(s(10), 1_300);
    assert_eq!(m.bytes_per_sec(), 125);
}

#[test]
fn eta_is_unknown_without_speed() {
    let t0 = Instant::now();
    let mut m = SpeedMeter::new(Duration::from_secs(3));
    m.record(t0, 50);
    m.record(t0 + Duration::from_secs(1), 50);
    assert_eq!(
        (m.bytes_per_sec(), m.eta(10), m.eta(0)),
        (0, None, Some(Duration::ZERO))
    );
}

#[test]
fn checkpoint_blocks_while_paused() {
    let c = Control::new();
    c.pause();
    assert!(c.is_paused());
    let (tx, rx) = mpsc::channel();
    let worker = {
        let c = c.clone();
        std::thread::spawn(move || tx.send(c.checkpoint().is_ok()).unwrap())
    };
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    c.resume();
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));
    worker.join().unwrap();
}

#[test]
fn cancel_wakes_a_paused_checkpoint_and_the_token() {
    let c = Control::new();
    let token = c.cancel_token();
    c.pause();
    let worker = {
        let c = c.clone();
        std::thread::spawn(move || c.checkpoint())
    };
    std::thread::sleep(Duration::from_millis(50));
    c.cancel();
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        ErrorKind::Cancelled
    );
    assert!(c.is_cancelled() && token.is_cancelled());
    c.resume();
    assert!(c.checkpoint().is_err());
}

#[test]
fn item_failure_keeps_kind_and_message() {
    let p = VPath::local(std::env::temp_dir()).unwrap();
    let e = rf_core::Error::io(
        "open_read",
        p.clone(),
        std::io::Error::from(std::io::ErrorKind::NotFound),
    );
    let f = ItemFailure::new(Stage::Read, Some(&p), None, &e);
    assert_eq!(
        (f.stage, f.kind, f.source.as_ref(), f.target.as_ref()),
        (Stage::Read, ErrorKind::NotFound, Some(&p), None)
    );
    assert_eq!(f.message, e.to_string());
}

#[test]
fn created_item_records_identity() {
    let mut e = Entry::new("a", EntryKind::File);
    (e.size, e.volume_id, e.file_id) = (7, Some(VolumeId(1)), Some(FileId(2)));
    let p = VPath::local(std::env::temp_dir()).unwrap();
    let want = CreatedItem {
        path: p.clone(),
        is_dir: false,
        size: 7,
        modified: None,
        identity: Some((1, 2)),
    };
    assert_eq!(CreatedItem::from_entry(p, &e), want);
}

#[test]
fn undo_records_roundtrip_and_count_items() {
    let p = |n: &str| VPath::local(std::env::temp_dir()).unwrap().join(n).unwrap();
    let rec = UndoRecord::Moved {
        moves: vec![MovedItem {
            from: p("a"),
            to: p("b"),
        }],
        created_dirs: vec![p("d")],
        removed_dirs: vec![],
    };
    assert_eq!(rec.item_count(), 1);
    let json = serde_json::to_string(&rec).unwrap();
    assert_eq!(serde_json::from_str::<UndoRecord>(&json).unwrap(), rec);
    let trashed = UndoRecord::Trashed {
        items: vec![
            TrashedItem {
                original: p("x"),
                location: None
            };
            3
        ],
    };
    assert_eq!(trashed.item_count(), 3);
    assert_eq!(
        UndoRecord::Renamed {
            from: p("a"),
            to: p("b")
        }
        .item_count(),
        1
    );
}

#[test]
fn speed_meter_with_a_window_longer_than_the_clock_keeps_every_sample() {
    // `now - window` would panic with "overflow when subtracting duration from instant".
    let t0 = Instant::now();
    let mut m = SpeedMeter::new(Duration::MAX);
    m.record(t0, 0);
    m.record(t0 + Duration::from_secs(1), 100);
    m.record(t0 + Duration::from_secs(2), 400);
    assert_eq!(m.bytes_per_sec(), 200);
}

#[test]
fn speed_meter_keeps_two_samples_after_a_long_pause() {
    let t0 = Instant::now();
    let mut m = SpeedMeter::new(Duration::from_secs(3));
    m.record(t0, 0);
    m.record(t0 + Duration::from_secs(60), 600);
    assert_eq!(m.bytes_per_sec(), 10);
    // Equal times, and a count that went back, both read as no speed.
    m.record(t0 + Duration::from_secs(60), 600);
    assert_eq!(m.bytes_per_sec(), 0);
    m.record(t0 + Duration::from_secs(61), 0);
    assert_eq!(m.bytes_per_sec(), 0);
}

#[test]
fn control_is_shared_between_threads_and_cancel_is_final() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Control>();

    let c = Control::new();
    assert!(!c.is_paused() && !c.is_cancelled());
    assert!(c.checkpoint().is_ok());
    c.cancel();
    c.pause();
    assert!(c.is_cancelled() && !c.is_paused());
    c.resume();
    assert!(c.is_cancelled());
}

#[test]
fn undo_identity_survives_json_at_full_width() {
    let mut e = Entry::new("a", EntryKind::Dir);
    (e.volume_id, e.file_id) = (Some(VolumeId(u64::MAX)), Some(FileId(u128::MAX)));
    let p = VPath::local(std::env::temp_dir()).unwrap();
    let rec = UndoRecord::Copied {
        created: vec![CreatedItem::from_entry(p, &e)],
        overwritten: 2,
        at: Some(std::time::SystemTime::now()),
    };
    let json = serde_json::to_string(&rec).unwrap();
    assert_eq!(serde_json::from_str::<UndoRecord>(&json).unwrap(), rec);
}
