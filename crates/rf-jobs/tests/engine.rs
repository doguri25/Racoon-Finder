#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use common::*;
use rf_jobs::testkit::{
    Fault, FaultyProvider, ScriptedResolver, temp_files, tree, wait_finished, wait_for,
};
use rf_jobs::{
    CreateKind, DefaultLanes, DeleteMethod, DeleteSpec, EngineConfig, ErrorPolicy, IncompleteJob,
    JobEngine, JobEvent, JobId, JobSnapshot, JobSpec, JobState, LaneId, LanePolicy, NonInteractive,
    Outcome, Stage,
};
use rf_vfs::Vfs;

const WAIT: Duration = Duration::from_secs(20);

fn engine(vfs: Vfs) -> (JobEngine, Receiver<JobEvent>) {
    JobEngine::start(EngineConfig::new(vfs, Arc::new(NonInteractive)))
}

fn one_file(f: &Fx, name: &str, len: u64) -> JobSpec {
    let p = f.src.join(name);
    std::fs::write(&p, pattern(len)).unwrap();
    copy(&[p], &f.dst)
}

fn finished_id(rx: &Receiver<JobEvent>) -> JobId {
    match wait_for(rx, WAIT, |e| matches!(e, JobEvent::Finished { .. })) {
        JobEvent::Finished { id, .. } => id,
        _ => unreachable!(),
    }
}

#[test]
fn a_submitted_job_runs_and_reports() {
    let f = fx();
    let (e, rx) = engine(Vfs::with_local());
    let id = e.submit(one_file(&f, "a.bin", 1 << 20));
    assert_eq!(wait_finished(&rx, id, WAIT).outcome, Outcome::Completed);
    assert_eq!(
        std::fs::read(f.dst.join("a.bin")).unwrap(),
        pattern(1 << 20)
    );
    assert_eq!(e.jobs(), Vec::<JobSnapshot>::new());
    assert!(e.shutdown(WAIT));
}

#[test]
fn main_lane_runs_jobs_one_at_a_time_in_order() {
    let f = fx();
    let (e, rx) = engine(Vfs::with_local());
    let ids: Vec<_> = (0..3)
        .map(|i| e.submit(one_file(&f, &format!("{i}.bin"), 1 << 20)))
        .collect();
    let (mut finished, mut running, mut max_running) = (Vec::new(), 0, 0);
    while finished.len() < 3 {
        match wait_for(&rx, WAIT, |_| true) {
            JobEvent::State {
                state: JobState::Planning,
                ..
            } => {
                running += 1;
                max_running = max_running.max(running);
            }
            JobEvent::Finished { id, .. } => {
                running -= 1;
                finished.push(id);
            }
            _ => {}
        }
    }
    assert_eq!((finished, max_running), (ids, 1));
    assert!(e.shutdown(WAIT));
}

#[test]
fn reordering_changes_what_runs_next() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, rx) = engine(p.vfs());
    let a = e.submit(one_file(&f, "a", 10));
    assert!(p.wait_held(WAIT));
    let b = e.submit(one_file(&f, "b", 10));
    let c = e.submit(one_file(&f, "c", 10));
    assert!(e.move_to(c, 0));
    assert!(!e.move_to(a, 0), "a running job cannot move");
    assert_eq!(e.jobs().iter().map(|j| j.id).collect::<Vec<_>>(), [a, c, b]);
    p.release();
    assert_eq!(
        [finished_id(&rx), finished_id(&rx), finished_id(&rx)],
        [a, c, b]
    );
    assert!(e.shutdown(WAIT));
}

#[test]
fn a_paused_queued_job_waits_until_resumed() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, rx) = engine(p.vfs());
    let a = e.submit(one_file(&f, "a", 10));
    assert!(p.wait_held(WAIT));
    let b = e.submit(one_file(&f, "b", 10));
    assert!(e.pause(b));
    p.release();
    wait_finished(&rx, a, WAIT);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        e.jobs().iter().map(|j| (j.id, j.state)).collect::<Vec<_>>(),
        [(b, JobState::Paused)]
    );
    assert!(!f.dst.join("b").exists());
    assert!(e.resume(b));
    assert_eq!(wait_finished(&rx, b, WAIT).outcome, Outcome::Completed);
    assert!(e.shutdown(WAIT));
}

#[test]
fn a_running_job_pauses_at_the_next_checkpoint() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, rx) = engine(p.vfs());
    let a = e.submit(one_file(&f, "a.bin", 4 << 20));
    assert!(p.wait_held(WAIT));
    assert!(e.pause(a));
    p.release();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(e.jobs()[0].state, JobState::Paused);
    assert!(!f.dst.join("a.bin").exists());
    assert!(e.resume(a));
    assert_eq!(wait_finished(&rx, a, WAIT).outcome, Outcome::Completed);
    assert_eq!(
        std::fs::read(f.dst.join("a.bin")).unwrap(),
        pattern(4 << 20)
    );
    assert!(e.shutdown(WAIT));
}

#[test]
fn cancel_works_for_queued_and_running_jobs() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, rx) = engine(p.vfs());
    let a = e.submit(one_file(&f, "a.bin", 4 << 20));
    assert!(p.wait_held(WAIT));
    let b = e.submit(one_file(&f, "b.bin", 10));
    assert!(e.cancel(b));
    assert_eq!(wait_finished(&rx, b, WAIT).outcome, Outcome::Cancelled);
    assert!(e.cancel(a));
    p.release();
    assert_eq!(wait_finished(&rx, a, WAIT).outcome, Outcome::Cancelled);
    assert_eq!(tree(&f.dst), BTreeMap::new());
    assert!(!e.cancel(a));
    assert!(e.shutdown(WAIT));
}

#[test]
fn renames_run_while_the_main_lane_is_busy() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, rx) = engine(p.vfs());
    let a = e.submit(one_file(&f, "a.bin", 10));
    assert!(p.wait_held(WAIT));
    std::fs::write(f.src.join("r1"), "r").unwrap();
    let r = e.submit(JobSpec::Rename {
        from: v(f.src.join("r1")),
        to: v(f.src.join("r2")),
    });
    assert_eq!(wait_finished(&rx, r, WAIT).outcome, Outcome::Completed);
    assert!(f.src.join("r2").exists());
    p.release();
    wait_finished(&rx, a, WAIT);
    assert!(e.shutdown(WAIT));
}

#[test]
fn engine_calls_return_while_providers_block() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, _rx) = engine(p.vfs());
    let a = e.submit(one_file(&f, "a.bin", 10));
    assert!(p.wait_held(WAIT));
    let spec_b = one_file(&f, "b.bin", 10);
    let started = Instant::now();
    let b = e.submit(spec_b);
    let _rx2 = e.subscribe();
    assert!(e.pause(b) && e.resume(b) && e.move_to(b, 0));
    let _ = e.jobs();
    assert!(e.pause(a) && e.cancel(b));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    p.release();
    assert!(e.resume(a));
    assert!(e.shutdown(WAIT));
}

#[test]
fn jobs_finish_with_no_window_listening() {
    let f = fx();
    let (e, rx) = engine(Vfs::with_local());
    drop(rx);
    e.submit(one_file(&f, "a.bin", 1 << 20));
    let deadline = Instant::now() + WAIT;
    while !f.dst.join("a.bin").exists() || !e.jobs().is_empty() {
        assert!(Instant::now() < deadline, "the job never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(e.shutdown(WAIT));
}

#[test]
fn progress_events_carry_totals() {
    let f = fx();
    let (e, rx) = engine(Vfs::with_local());
    let id = e.submit(one_file(&f, "a.bin", 8 << 20));
    let mut last = None;
    loop {
        match wait_for(&rx, WAIT, |_| true) {
            JobEvent::Progress { id: pid, progress } if pid == id => last = Some(progress),
            JobEvent::Finished { id: fid, .. } if fid == id => break,
            _ => {}
        }
    }
    let last = last.expect("progress");
    assert_eq!((last.done.bytes, last.total.bytes), (8 << 20, 8 << 20));
    assert!(e.shutdown(WAIT));
}

#[test]
fn shutdown_cancels_running_and_queued_jobs() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let (e, rx) = engine(p.vfs());
    let a = e.submit(one_file(&f, "a.bin", 4 << 20));
    assert!(p.wait_held(WAIT));
    let b = e.submit(one_file(&f, "b.bin", 10));
    let releaser = {
        let p = Arc::clone(&p);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            p.release();
        })
    };
    assert!(e.shutdown(WAIT));
    releaser.join().unwrap();
    let mut outcomes = BTreeMap::new();
    while let Ok(ev) = rx.try_recv() {
        if let JobEvent::Finished { id, report } = ev {
            outcomes.insert(id, report.outcome);
        }
    }
    assert_eq!(
        (outcomes.get(&a), outcomes.get(&b)),
        (Some(&Outcome::Cancelled), Some(&Outcome::Cancelled))
    );
    assert!(temp_files(&f.dst).is_empty() && !f.dst.join("a.bin").exists());
}

/// The first engine is abandoned on purpose: its worker never returns, like a killed process.
fn crash_mid_file(f: &Fx, journal: &Path) -> JobId {
    let p = FaultyProvider::local(vec![Fault::Crash {
        file: 0,
        after: 1 << 20,
    }]);
    let (e, _rx) = JobEngine::start(
        EngineConfig::new(p.vfs(), Arc::new(NonInteractive)).with_journal(journal),
    );
    let id = e.submit(one_file(f, "a.bin", 4 << 20));
    assert!(p.wait_crashed(WAIT));
    assert!(!f.dst.join("a.bin").exists());
    assert_eq!(temp_files(&f.dst).len(), 1);
    id
}

fn restart(journal: &Path) -> (JobEngine, Receiver<JobEvent>, Vec<IncompleteJob>, usize) {
    let (e, rx) = JobEngine::start(
        EngineConfig::new(Vfs::with_local(), Arc::new(NonInteractive)).with_journal(journal),
    );
    match wait_for(&rx, WAIT, |ev| matches!(ev, JobEvent::Recovered { .. })) {
        JobEvent::Recovered {
            jobs,
            temp_files_removed,
        } => (e, rx, jobs, temp_files_removed),
        _ => unreachable!(),
    }
}

#[test]
fn a_crash_mid_file_is_recovered_on_the_next_start() {
    let f = fx();
    let journal = f.root.join("state");
    let id = crash_mid_file(&f, &journal);
    let (e, rx, jobs, removed) = restart(&journal);
    assert_eq!(
        jobs.iter().map(|j| (j.id, j.started)).collect::<Vec<_>>(),
        [(id, true)]
    );
    assert_eq!(removed, 1);
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
    assert_eq!(
        std::fs::read(f.src.join("a.bin")).unwrap(),
        pattern(4 << 20)
    );
    assert_eq!(e.recovered().len(), 1);
    let again = e.resubmit_recovered(id).expect("resubmitted");
    assert_eq!(wait_finished(&rx, again, WAIT).outcome, Outcome::Completed);
    assert_eq!(
        std::fs::read(f.dst.join("a.bin")).unwrap(),
        pattern(4 << 20)
    );
    assert_eq!(e.recovered(), Vec::<IncompleteJob>::new());
    assert!(e.shutdown(WAIT));
    let (e3, _, jobs3, _) = restart(&journal);
    assert_eq!(jobs3, Vec::<IncompleteJob>::new());
    assert!(e3.shutdown(WAIT));
}

#[test]
fn a_dismissed_recovery_is_forgotten() {
    let f = fx();
    let journal = f.root.join("state");
    let id = crash_mid_file(&f, &journal);
    let (e, _, _, _) = restart(&journal);
    assert!(e.dismiss_recovered(id));
    assert!(!e.dismiss_recovered(id));
    assert!(e.shutdown(WAIT));
    let (e2, _, jobs, _) = restart(&journal);
    assert_eq!(jobs, Vec::<IncompleteJob>::new());
    assert!(e2.shutdown(WAIT));
}

#[test]
fn a_panic_in_a_job_does_not_stop_the_lane() {
    let f = fx();
    std::fs::write(f.dst.join("a.bin"), "old").unwrap();
    let (e, rx) = JobEngine::start(EngineConfig::new(
        Vfs::with_local(),
        Arc::new(ScriptedResolver::new()),
    ));
    let first = e.submit(one_file(&f, "a.bin", 10));
    let rep = wait_finished(&rx, first, WAIT);
    assert_eq!(
        (rep.outcome, rep.failures[0].stage),
        (Outcome::Aborted, Stage::Internal)
    );
    let second = e.submit(one_file(&f, "b.bin", 10));
    assert_eq!(wait_finished(&rx, second, WAIT).outcome, Outcome::Completed);
    assert!(e.shutdown(WAIT));
}

#[test]
fn default_lanes_keep_quick_jobs_off_the_main_lane() {
    let f = fx();
    assert_eq!(
        DefaultLanes.lane(&JobSpec::Rename {
            from: v(&f.src),
            to: v(&f.dst)
        }),
        LaneId::QUICK
    );
    assert_eq!(
        DefaultLanes.lane(&JobSpec::Create {
            parent: v(&f.src),
            name: "x".into(),
            kind: CreateKind::Dir
        }),
        LaneId::QUICK
    );
    assert_eq!(DefaultLanes.lane(&copy(&[], &f.dst)), LaneId::MAIN);
    let del = JobSpec::Delete(DeleteSpec {
        paths: vec![],
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    });
    assert_eq!(DefaultLanes.lane(&del), LaneId::MAIN);
}

#[derive(Debug)]
struct EachJobItsOwnLane(AtomicU32);

impl LanePolicy for EachJobItsOwnLane {
    fn lane(&self, _: &JobSpec) -> LaneId {
        LaneId(10 + self.0.fetch_add(1, Ordering::SeqCst))
    }
}

#[test]
fn a_lane_policy_can_run_jobs_in_parallel() {
    let f = fx();
    let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
    let mut config = EngineConfig::new(p.vfs(), Arc::new(NonInteractive));
    config.lanes = Arc::new(EachJobItsOwnLane(AtomicU32::new(0)));
    let (e, rx) = JobEngine::start(config);
    let a = e.submit(one_file(&f, "a", 10));
    assert!(p.wait_held(WAIT));
    let b = e.submit(one_file(&f, "b", 10));
    assert_eq!(wait_finished(&rx, b, WAIT).outcome, Outcome::Completed);
    p.release();
    wait_finished(&rx, a, WAIT);
    assert!(e.shutdown(WAIT));
}
