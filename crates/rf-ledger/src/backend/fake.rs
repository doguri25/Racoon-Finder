//! An in-memory [`SystemBackend`] for tests: live slots, a call log and fault injection.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::SystemBackend;
use crate::change::{Before, Change, Target, Value};
use crate::error::LedgerError;

/// A [`SystemBackend`] method, as recorded in the call log and named by a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeOp {
    Capture,
    Apply,
    Restore,
    IsApplied,
    ClearDomain,
}

/// What an injected fault does when its call comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// The call fails without any effect.
    Fail,
    /// The call takes effect and then panics, like a process that dies right after the change.
    PanicAfter,
}

#[derive(Debug, Default)]
struct State {
    live: BTreeMap<Target, Value>,
    calls: Vec<FakeOp>,
    faults: Vec<(FakeOp, Target, Fault)>,
    /// The keys a capture of a change that sets the target reports as created.
    created_keys: BTreeMap<Target, Vec<String>>,
    /// The before-state of every `restore` call, oldest first.
    restored: Vec<Before>,
}

/// A [`SystemBackend`] that keeps its slots in memory.
///
/// Clones share one state, so a test hands one clone to the ledger and inspects the other.
/// A slot is identified by the exact [`Target`]; the case rules of registry names belong to
/// the ledger (`Target::same_slot`), not to this fake. Any kind of target is accepted. The fake
/// keeps no registry keys: a capture reports the keys [`FakeBackend::create_keys`] names, and
/// [`FakeBackend::restored`] shows which keys each `restore` was asked to remove.
#[derive(Clone, Debug, Default)]
pub struct FakeBackend {
    state: Arc<Mutex<State>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets (`Some`) or removes (`None`) a slot directly, as if another program had changed it.
    /// Not recorded in the call log.
    pub fn set(&self, target: &Target, value: Option<Value>) {
        let mut state = self.lock();
        match value {
            Some(value) => state.live.insert(target.clone(), value),
            None => state.live.remove(target),
        };
    }

    /// The live value of a slot, or `None` if it does not exist.
    pub fn get(&self, target: &Target) -> Option<Value> {
        self.lock().live.get(target).cloned()
    }

    /// Every trait method called so far, oldest first, whatever its result.
    pub fn calls(&self) -> Vec<FakeOp> {
        self.lock().calls.clone()
    }

    /// Makes every later `capture` of a change that sets `target` (not one that removes it)
    /// report `keys` as the registry keys `apply` creates, the way the registry backend reports
    /// the keys missing on the way to a value (canonical form, shallowest first).
    pub fn create_keys(&self, target: &Target, keys: &[&str]) {
        let keys = keys.iter().map(|&key| key.to_owned()).collect();
        self.lock().created_keys.insert(target.clone(), keys);
    }

    /// The before-state every `restore` call was given so far, oldest first, whatever its
    /// result: its `created_keys` are the keys that call was asked to remove.
    pub fn restored(&self) -> Vec<Before> {
        self.lock().restored.clone()
    }

    /// Makes the next `op` on `target` fail with [`LedgerError::Backend`] and no effect.
    pub fn fail_next(&self, op: FakeOp, target: &Target) {
        self.lock().faults.push((op, target.clone(), Fault::Fail));
    }

    /// Makes the next `op` on `target` take effect and then panic with `injected crash`.
    pub fn panic_after_next(&self, op: FakeOp, target: &Target) {
        self.lock()
            .faults
            .push((op, target.clone(), Fault::PanicAfter));
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One trait call: logs it, applies a due fault, otherwise runs `effect` on the live slots.
    fn step<T>(
        &self,
        op: FakeOp,
        target: &Target,
        effect: impl FnOnce(&mut BTreeMap<Target, Value>) -> T,
    ) -> Result<T, LedgerError> {
        let mut state = self.lock();
        state.calls.push(op);
        let due = state
            .faults
            .iter()
            .position(|(fault_op, fault_target, _)| *fault_op == op && fault_target == target);
        let fault = due.map(|index| state.faults.remove(index).2);
        if fault == Some(Fault::Fail) {
            return Err(LedgerError::Backend("injected failure".into()));
        }
        let out = effect(&mut state.live);
        // Release the lock first so the panic leaves the shared state usable.
        drop(state);
        assert!(fault != Some(Fault::PanicAfter), "injected crash");
        Ok(out)
    }
}

/// Overwrites `slot` with `value`, or removes it for `None`.
fn put(live: &mut BTreeMap<Target, Value>, slot: &Target, value: Option<&Value>) {
    match value {
        Some(value) => live.insert(slot.clone(), value.clone()),
        None => live.remove(slot),
    };
}

impl SystemBackend for FakeBackend {
    fn capture(&mut self, change: &Change) -> Result<Before, LedgerError> {
        let created_keys = match change.value {
            Some(_) => self.lock().created_keys.get(&change.target).cloned(),
            None => None,
        };
        self.step(FakeOp::Capture, &change.target, |live| Before {
            value: live.get(&change.target).cloned(),
            created_keys: created_keys.unwrap_or_default(),
        })
    }

    fn apply(&mut self, change: &Change) -> Result<(), LedgerError> {
        self.step(FakeOp::Apply, &change.target, |live| {
            put(live, &change.target, change.value.as_ref());
        })
    }

    fn restore(&mut self, change: &Change, before: &Before) -> Result<(), LedgerError> {
        self.lock().restored.push(before.clone());
        self.step(FakeOp::Restore, &change.target, |live| {
            put(live, &change.target, before.value.as_ref());
        })
    }

    fn is_applied(&mut self, change: &Change) -> Result<bool, LedgerError> {
        self.step(FakeOp::IsApplied, &change.target, |live| {
            live.get(&change.target) == change.value.as_ref()
        })
    }

    fn clear_domain(&mut self, _domain: &str) -> Result<(), LedgerError> {
        self.lock().calls.push(FakeOp::ClearDomain);
        Ok(())
    }
}
