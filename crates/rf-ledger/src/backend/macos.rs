#![allow(
    unsafe_code,
    reason = "CoreFoundation preferences FFI: CFPreferencesSetValue and CFPropertyList conversion are unsafe"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "rf-ledger's preferences backend is the only code allowed to write preferences (PRD INST-02)"
)]

//! The macOS backend: preferences (`defaults`) of the current user, any host.

use std::ptr;

use objc2_core_foundation::{
    CFArray, CFBoolean, CFData, CFGetTypeID, CFNumber, CFPreferencesCopyKeyList,
    CFPreferencesCopyValue, CFPreferencesSetValue, CFPreferencesSynchronize,
    CFPropertyListCreateData, CFPropertyListCreateWithData, CFPropertyListFormat, CFRetained,
    CFString, CFType, Type, kCFPreferencesAnyApplication, kCFPreferencesAnyHost,
    kCFPreferencesCurrentUser,
};
use rf_core::identity::BUNDLE_ID;

use super::SystemBackend;
use crate::change::{Before, Change, GLOBAL_DOMAIN, Target, Value};
use crate::error::LedgerError;

/// The [`SystemBackend`] of macOS: preferences of the current user for any host, the scope
/// `defaults read` and `defaults write` use. `NSGlobalDomain` is the domain of every application
/// (`defaults -g`).
///
/// Every method checks its change again before it reads or writes a preference, the before-value
/// included, because both may come from the ledger file. A previous value is always captured as
/// [`Value::Plist`], a binary property list, so a revert puts back a value of any type
/// (dictionaries, arrays and dates included) without loss. Every write is synchronized before the
/// method returns, and every read first synchronizes, so it sees what other processes wrote.
/// Registry values and services are [`LedgerError::Unsupported`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PreferencesBackend;

/// The current user and any host: the scope `defaults` reads and writes.
fn scope() -> (&'static CFString, &'static CFString) {
    // SAFETY: CoreFoundation exports both as immutable `CFString` constants that are set before
    // any code of this process runs; reading them has no side effect.
    unsafe { (kCFPreferencesCurrentUser, kCFPreferencesAnyHost) }
}

/// The application ID `CFPreferences` takes for `domain`.
fn application_id(domain: &str) -> CFRetained<CFString> {
    if domain == GLOBAL_DOMAIN {
        // SAFETY: as in `scope`, an immutable `CFString` constant of CoreFoundation.
        unsafe { kCFPreferencesAnyApplication }.retain()
    } else {
        CFString::from_str(domain)
    }
}

/// Writes the pending changes of `domain` and reads it again from the preferences daemon.
fn synchronize(app: &CFString, domain: &str) -> Result<(), LedgerError> {
    let (user, host) = scope();
    if CFPreferencesSynchronize(app, user, host) {
        Ok(())
    } else {
        Err(LedgerError::Backend(format!(
            "CFPreferencesSynchronize failed for {domain}"
        )))
    }
}

/// One preference: a key of a domain.
struct Slot<'a> {
    domain: &'a str,
    app: CFRetained<CFString>,
    key: CFRetained<CFString>,
}

impl<'a> Slot<'a> {
    /// Checks `change` and returns the preference it targets.
    fn of(change: &'a Change) -> Result<Self, LedgerError> {
        change.validate()?;
        match &change.target {
            Target::Preference { domain, key } => Ok(Self {
                domain,
                app: application_id(domain),
                key: CFString::from_str(key),
            }),
            other => Err(LedgerError::Unsupported(format!(
                "{other} is not a macOS preference"
            ))),
        }
    }

    /// The live value, or `None` if the key does not exist.
    fn read(&self) -> Result<Option<CFRetained<CFType>>, LedgerError> {
        synchronize(&self.app, self.domain)?;
        let (user, host) = scope();
        Ok(CFPreferencesCopyValue(&self.key, &self.app, user, host))
    }

    /// Sets the key to `value` (`None` removes it) and synchronizes the domain.
    fn write(&self, value: Option<&CFType>) -> Result<(), LedgerError> {
        let (user, host) = scope();
        // SAFETY: `value` is a property list object (`to_cf` makes nothing else), the only kind
        // of value `CFPreferencesSetValue` takes; `None` removes the key.
        unsafe { CFPreferencesSetValue(&self.key, value, &self.app, user, host) };
        synchronize(&self.app, self.domain)
    }
}

/// The property list object for `value`; values a preference cannot hold are
/// [`LedgerError::InvalidValue`].
fn to_cf(value: &Value) -> Result<CFRetained<CFType>, LedgerError> {
    Ok(match value {
        Value::Str(s) => CFString::from_str(s).into(),
        Value::Bool(b) => CFRetained::from(CFBoolean::new(*b)),
        Value::I64(n) => CFNumber::new_i64(*n).into(),
        Value::Bytes(bytes) => CFData::from_bytes(bytes).into(),
        Value::Plist(bytes) => {
            let data = CFData::from_bytes(bytes);
            // SAFETY: `data` is a live `CFData`; the default allocator (`None`) and NULL for the
            // format and error out-pointers (meaning "not wanted") are allowed.
            unsafe {
                CFPropertyListCreateWithData(None, Some(&data), 0, ptr::null_mut(), ptr::null_mut())
            }
            .ok_or_else(|| LedgerError::InvalidValue("not a property list".into()))?
        }
        other => {
            return Err(LedgerError::InvalidValue(format!(
                "{other:?} cannot be stored in macOS preferences"
            )));
        }
    })
}

/// Whether `a` and `b` are the same value, type included.
///
/// `CFEqual` alone is not enough: it may find an integer equal to a boolean or to a real of the
/// same value. Inside a dictionary or an array the elements compare by `CFEqual` alone.
fn same_value(a: &CFType, b: &CFType) -> bool {
    if CFGetTypeID(Some(a)) != CFGetTypeID(Some(b)) {
        return false;
    }
    if let (Some(x), Some(y)) = (a.downcast_ref::<CFNumber>(), b.downcast_ref::<CFNumber>()) {
        if x.is_float_type() != y.is_float_type() {
            return false;
        }
    }
    a == b
}

impl SystemBackend for PreferencesBackend {
    fn capture(&mut self, change: &Change) -> Result<Before, LedgerError> {
        let slot = Slot::of(change)?;
        let Some(live) = slot.read()? else {
            return Ok(Before::default());
        };
        // SAFETY: `live` is the property list object `CFPreferencesCopyValue` returned; the default
        // allocator (`None`) and a NULL error out-pointer (meaning "not wanted") are allowed.
        let data = unsafe {
            CFPropertyListCreateData(
                None,
                Some(&live),
                CFPropertyListFormat::BinaryFormat_v1_0,
                0,
                ptr::null_mut(),
            )
        }
        .ok_or_else(|| LedgerError::Backend(format!("cannot serialize {}", change.target)))?;
        Ok(Before {
            value: Some(Value::Plist(data.to_vec())),
            created_keys: Vec::new(),
        })
    }

    fn apply(&mut self, change: &Change) -> Result<(), LedgerError> {
        let slot = Slot::of(change)?;
        let value = change.value.as_ref().map(to_cf).transpose()?;
        slot.write(value.as_deref())
    }

    /// `before.created_keys` is not used: a preference has no keys to create.
    fn restore(&mut self, change: &Change, before: &Before) -> Result<(), LedgerError> {
        let slot = Slot::of(change)?;
        let value = before.value.as_ref().map(to_cf).transpose()?;
        slot.write(value.as_deref())
    }

    fn is_applied(&mut self, change: &Change) -> Result<bool, LedgerError> {
        let slot = Slot::of(change)?;
        let wanted = change.value.as_ref().map(to_cf).transpose()?;
        Ok(match (slot.read()?, wanted) {
            (None, None) => true,
            (Some(live), Some(wanted)) => same_value(&live, &wanted),
            _ => false,
        })
    }

    /// Removes every key of `domain`, which must be the app's own: its bundle ID, or a domain
    /// below it (`<bundle ID>.…`). Any other domain, the global one included, is
    /// [`LedgerError::InvalidTarget`].
    fn clear_domain(&mut self, domain: &str) -> Result<(), LedgerError> {
        Change::preference(domain, "x", None)?;
        let owned = domain == BUNDLE_ID
            || domain
                .strip_prefix(BUNDLE_ID)
                .is_some_and(|rest| rest.starts_with('.'));
        if !owned {
            return Err(LedgerError::InvalidTarget(domain.to_owned()));
        }
        let app = CFString::from_str(domain);
        synchronize(&app, domain)?;
        let (user, host) = scope();
        let Some(keys) = CFPreferencesCopyKeyList(&app, user, host) else {
            return Ok(());
        };
        // SAFETY: the keys of a preferences domain are strings, so the array holds `CFString`s.
        let keys = unsafe { CFRetained::cast_unchecked::<CFArray<CFString>>(keys) };
        for key in keys.iter() {
            // SAFETY: a NULL value (`None`) removes the key.
            unsafe { CFPreferencesSetValue(&key, None, &app, user, host) };
        }
        synchronize(&app, domain)
    }
}
