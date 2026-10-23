#![expect(
    clippy::disallowed_methods,
    reason = "rf-ledger's registry backend is the only code allowed to write the registry (PRD INST-02)"
)]

//! The Windows backend: registry values under `HKEY_CURRENT_USER`, and nothing else (WINX-06).

use std::fmt;

use windows_registry::{CURRENT_USER, Key, Type};

use super::SystemBackend;
use crate::change::{Before, Change, Target, Value, hkcu_subkey, registry_data};
use crate::error::LedgerError;

/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`, 0x80070002: the key or value does not exist.
const FILE_NOT_FOUND: i32 = -2_147_024_894;
/// `HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND)`, 0x80070003: a key on the way does not exist.
const PATH_NOT_FOUND: i32 = -2_147_024_893;

/// The [`SystemBackend`] of Windows: values in the registry of the current user.
///
/// Every method checks its change again before it calls the registry, the keys a
/// [`Before`] lists included, because both may come from the ledger file: a tampered file
/// must not reach another hive. A previous value is always captured as [`Value::Raw`], the
/// type code and bytes exactly as stored, so a revert puts back a value of any type
/// (`REG_NONE` included) without loss. Preferences and services are
/// [`LedgerError::Unsupported`].
#[derive(Clone, Copy, Debug, Default)]
pub struct RegistryBackend;

/// Checks `change` and returns the key below `HKEY_CURRENT_USER` and the value name it targets.
fn registry_slot(change: &Change) -> Result<(&str, &str), LedgerError> {
    change.validate()?;
    match &change.target {
        Target::Registry { key, name } => Ok((hkcu_subkey(key)?, name)),
        other => Err(LedgerError::Unsupported(format!(
            "{other} is not a registry value"
        ))),
    }
}

/// `Ok(None)` when the registry says the key or value does not exist: a state, not a failure.
fn found<T>(result: windows_registry::Result<T>) -> windows_registry::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if matches!(e.code().0, FILE_NOT_FOUND | PATH_NOT_FOUND) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A failed call on the key `HKCU\{subkey}`.
fn key_failed(op: &str, subkey: &str, e: &impl fmt::Display) -> LedgerError {
    LedgerError::Backend(format!(r"{op} HKCU\{subkey}: {e}"))
}

/// A failed call on the value `name` of the key `HKCU\{subkey}`.
fn value_failed(op: &str, subkey: &str, name: &str, e: &impl fmt::Display) -> LedgerError {
    let name = if name.is_empty() { "(default)" } else { name };
    LedgerError::Backend(format!(r"{op} HKCU\{subkey} @ {name}: {e}"))
}

/// Opens `HKCU\{subkey}` for reading, or `None` if it does not exist.
fn open_key(subkey: &str) -> Result<Option<Key>, LedgerError> {
    found(CURRENT_USER.open(subkey)).map_err(|e| key_failed("open", subkey, &e))
}

/// The type code and bytes of the value `name` of `HKCU\{subkey}`, or `None` if the key or the
/// value does not exist.
fn read(subkey: &str, name: &str) -> Result<Option<(u32, Vec<u8>)>, LedgerError> {
    let Some(key) = open_key(subkey)? else {
        return Ok(None);
    };
    let value = found(key.get_value(name)).map_err(|e| value_failed("read", subkey, name, &e))?;
    Ok(value.map(|v| (u32::from(v.ty()), v.to_vec())))
}

/// Sets the value `name` of `HKCU\{subkey}` to exactly `ty` and `data`, creating every missing
/// key on the way.
fn write(subkey: &str, name: &str, ty: u32, data: &[u8]) -> Result<(), LedgerError> {
    // For a string type `RegSetValueExW` looks at the last UTF-16 unit of the data and at the
    // unit just past it, and stores one unit more than it was given when the data lacks its
    // terminating NUL but the next unit is NUL (with a single byte of data, "the last unit"
    // starts before the data). A non-NUL unit on each side keeps those reads inside this
    // buffer and the stored bytes exactly `data`, even for a string another program stored
    // without its terminator.
    let mut padded = Vec::with_capacity(data.len() + 4);
    padded.extend_from_slice(&[0xFF, 0xFF]);
    padded.extend_from_slice(data);
    padded.extend_from_slice(&[0xFF, 0xFF]);
    let data = &padded[2..2 + data.len()];
    let key = CURRENT_USER
        .create(subkey)
        .map_err(|e| key_failed("create", subkey, &e))?;
    // `Type::Other` carries any code through unchanged (`u32::from(Type::Other(ty)) == ty`).
    // `Type::from` would name the string codes, and for those `set_bytes` asserts in debug
    // builds that the data ends with a NUL unit: such a value could not be put back there.
    key.set_bytes(name, Type::Other(ty), data)
        .map_err(|e| value_failed("write", subkey, name, &e))
}

/// Deletes the value `name` of `HKCU\{subkey}`. A missing key or value is already the wanted
/// state, and a missing key is never created.
fn remove(subkey: &str, name: &str) -> Result<(), LedgerError> {
    // Deleting a value needs `KEY_SET_VALUE`, which the read-only `open` does not ask for.
    let key = found(CURRENT_USER.options().read().write().open(subkey))
        .map_err(|e| key_failed("open", subkey, &e))?;
    if let Some(key) = key {
        found(key.remove_value(name)).map_err(|e| value_failed("remove", subkey, name, &e))?;
    }
    Ok(())
}

/// Deletes `HKCU\{subkey}` if it holds no key and no value; a missing key counts as deleted.
/// Returns `false`, deleting nothing, when it holds something: that is someone else's.
fn remove_if_empty(subkey: &str) -> Result<bool, LedgerError> {
    let Some(key) = open_key(subkey)? else {
        return Ok(true);
    };
    let list_failed = |e| key_failed("list", subkey, &e);
    let empty = key.keys().map_err(list_failed)?.next().is_none()
        && key.values().map_err(list_failed)?.next().is_none();
    drop(key);
    if empty {
        found(CURRENT_USER.remove_tree(subkey)).map_err(|e| key_failed("delete", subkey, &e))?;
    }
    Ok(empty)
}

impl SystemBackend for RegistryBackend {
    fn capture(&mut self, change: &Change) -> Result<Before, LedgerError> {
        let (subkey, name) = registry_slot(change)?;
        let value = read(subkey, name)?.map(|(ty, data)| Value::Raw { ty, data });
        let mut created_keys = Vec::new();
        if change.value.is_some() {
            // `apply` creates every key from the first missing one down to `subkey`.
            let ends = subkey.match_indices('\\').map(|(end, _)| end);
            for prefix in ends.chain([subkey.len()]).map(|end| &subkey[..end]) {
                if !created_keys.is_empty() || open_key(prefix)?.is_none() {
                    created_keys.push(format!(r"HKCU\{prefix}"));
                }
            }
        }
        Ok(Before {
            value,
            created_keys,
        })
    }

    fn apply(&mut self, change: &Change) -> Result<(), LedgerError> {
        let (subkey, name) = registry_slot(change)?;
        match &change.value {
            Some(value) => {
                let (ty, data) = registry_data(value)?;
                write(subkey, name, ty, &data)
            }
            None => remove(subkey, name),
        }
    }

    fn restore(&mut self, change: &Change, before: &Before) -> Result<(), LedgerError> {
        let (subkey, name) = registry_slot(change)?;
        let created = before
            .created_keys
            .iter()
            .map(|key| hkcu_subkey(key))
            .collect::<Result<Vec<_>, _>>()?;
        match before.value.as_ref().map(registry_data).transpose()? {
            Some((ty, data)) => write(subkey, name, ty, &data)?,
            None => remove(subkey, name)?,
        }
        // Deepest first. A key that is not empty now holds something that is not ours, so it
        // stays, and so does every key above it.
        for key in created.into_iter().rev() {
            if !remove_if_empty(key)? {
                break;
            }
        }
        Ok(())
    }

    fn is_applied(&mut self, change: &Change) -> Result<bool, LedgerError> {
        let (subkey, name) = registry_slot(change)?;
        let wanted = change.value.as_ref().map(registry_data).transpose()?;
        Ok(read(subkey, name)? == wanted)
    }
}
