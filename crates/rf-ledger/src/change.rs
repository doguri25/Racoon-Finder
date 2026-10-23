//! The change model: what the app may change on the system, and the value it writes.
//!
//! The serde attributes on these types define the ledger file format, so they are part of
//! the on-disk contract (see the `json_shape_is_stable` test).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::LedgerError;

/// The `defaults` domain of settings every application reads (`NSGlobalDomain`).
pub const GLOBAL_DOMAIN: &str = "NSGlobalDomain";
/// The `NSGlobalDomain` key naming the app that opens folders from other apps ("Show in Finder").
pub const FILE_VIEWER_KEY: &str = "NSFileViewer";

/// Longest registry key component, in UTF-16 code units.
const MAX_KEY_COMPONENT_UNITS: usize = 255;
/// Longest registry value name, in UTF-16 code units.
const MAX_VALUE_NAME_UNITS: usize = 16_383;

/// Kind of background service a [`Target::Service`] names.
///
/// Reserved for the indexing service (IDX-05); no backend handles it yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    WindowsService,
    LaunchAgent,
}

impl ServiceKind {
    /// The name used in the ledger file and in log lines.
    const fn name(self) -> &'static str {
        match self {
            Self::WindowsService => "windows_service",
            Self::LaunchAgent => "launch_agent",
        }
    }
}

/// One slot of operating system state the app can change.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Target {
    /// A Windows registry value. `key` is in canonical form `HKCU\<subkey>`; an empty `name`
    /// is the key's default value.
    Registry { key: String, name: String },
    /// A macOS preference (`defaults`). `domain` is `NSGlobalDomain` or a bundle ID; the scope
    /// is the current user, any host (the default scope of `defaults`).
    Preference { domain: String, key: String },
    /// A background service. Reserved for IDX-05: no backend handles it yet.
    Service { service: ServiceKind, name: String },
}

/// A value the app writes to, or reads back from, a [`Target`].
///
/// "Remove the setting" is `Change::value == None`, not a variant of this enum. [`Value::Raw`]
/// and [`Value::Plist`] carry a previous value the backend cannot express any other way, so a
/// revert restores it without loss.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Value {
    Str(String),
    ExpandStr(String),
    MultiStr(Vec<String>),
    U32(u32),
    U64(u64),
    Bytes(Vec<u8>),
    /// A registry value of any type, as its type code and bytes.
    Raw {
        ty: u32,
        data: Vec<u8>,
    },
    Bool(bool),
    I64(i64),
    /// A property list value of a type other than string, bool or integer, serialized.
    Plist(Vec<u8>),
    /// What a service runs. Reserved for IDX-05: no backend handles it yet.
    Program {
        path: String,
        args: Vec<String>,
    },
}

/// A target and the value to set it to (`None` removes the setting).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub target: Target,
    pub value: Option<Value>,
}

/// What a target held before the app changed it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    /// The previous value, or `None` if the target did not exist.
    pub value: Option<Value>,
    /// Registry keys that `apply` created on the way (canonical form, shallowest first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_keys: Vec<String>,
}

/// Number of UTF-16 code units of `s`, the unit the Windows registry limits are written in.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Checks that `key` lives under `HKEY_CURRENT_USER` and returns the part after the hive.
///
/// The hive may be written `HKCU` or `HKEY_CURRENT_USER` in any ASCII case. Every other hive
/// is refused, `HKCR` included: it writes to `HKLM` when `HKCU` has no such key.
pub fn hkcu_subkey(key: &str) -> Result<&str, LedgerError> {
    let (hive, rest) = key.split_once('\\').unwrap_or((key, ""));
    if !(hive.eq_ignore_ascii_case("HKCU") || hive.eq_ignore_ascii_case("HKEY_CURRENT_USER")) {
        return Err(LedgerError::OutsideHkcu(key.to_owned()));
    }
    let well_formed = !rest.is_empty()
        && rest.split('\\').all(|part| {
            !part.is_empty() && !part.contains('\0') && utf16_len(part) <= MAX_KEY_COMPONENT_UNITS
        });
    if well_formed {
        Ok(rest)
    } else {
        Err(LedgerError::InvalidTarget(key.to_owned()))
    }
}

/// The registry type code and data bytes of `value`, exactly as the Windows registry stores it.
///
/// Strings are UTF-16LE with one terminating NUL unit; a multi-string ends every element with
/// a NUL unit plus one more at the end. These are the bytes `set_string` and `set_multi_string`
/// of the `windows-registry` crate write.
pub fn registry_data(value: &Value) -> Result<(u32, Vec<u8>), LedgerError> {
    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;
    const REG_BINARY: u32 = 3;
    const REG_DWORD: u32 = 4;
    const REG_MULTI_SZ: u32 = 7;
    const REG_QWORD: u32 = 11;

    fn utf16le(units: impl Iterator<Item = u16>) -> Vec<u8> {
        units.flat_map(u16::to_le_bytes).collect()
    }

    Ok(match value {
        Value::Str(s) => (REG_SZ, utf16le(s.encode_utf16().chain([0]))),
        Value::ExpandStr(s) => (REG_EXPAND_SZ, utf16le(s.encode_utf16().chain([0]))),
        Value::MultiStr(items) => {
            let units = items
                .iter()
                .flat_map(|item| item.encode_utf16().chain([0]))
                .chain([0]);
            (REG_MULTI_SZ, utf16le(units))
        }
        Value::U32(n) => (REG_DWORD, n.to_le_bytes().to_vec()),
        Value::U64(n) => (REG_QWORD, n.to_le_bytes().to_vec()),
        Value::Bytes(bytes) => (REG_BINARY, bytes.clone()),
        Value::Raw { ty, data } => (*ty, data.clone()),
        other => {
            return Err(LedgerError::InvalidValue(format!(
                "{other:?} cannot be stored in the registry"
            )));
        }
    })
}

impl Target {
    /// Whether `other` is the same slot of system state, which only one change group may own.
    ///
    /// Registry keys and value names are case-insensitive, so they compare case-insensitively.
    /// Of a registry key only the part after the hive counts: `HKCU\X` and
    /// `HKEY_CURRENT_USER\X` are one slot. Everything else must match exactly.
    pub fn same_slot(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Registry {
                    key: key_a,
                    name: name_a,
                },
                Self::Registry {
                    key: key_b,
                    name: name_b,
                },
            ) => {
                let same_key = match (hkcu_subkey(key_a), hkcu_subkey(key_b)) {
                    (Ok(a), Ok(b)) => a.to_lowercase() == b.to_lowercase(),
                    // Not an `HKCU` key (nothing can be applied to it): the text must match.
                    _ => key_a.to_lowercase() == key_b.to_lowercase(),
                };
                same_key && name_a.to_lowercase() == name_b.to_lowercase()
            }
            (a, b) => a == b,
        }
    }

    fn validate(&self) -> Result<(), LedgerError> {
        match self {
            Self::Registry { key, name } => {
                hkcu_subkey(key)?;
                if name.contains('\0') || utf16_len(name) > MAX_VALUE_NAME_UNITS {
                    return Err(LedgerError::InvalidTarget(name.clone()));
                }
            }
            Self::Preference { domain, key } => {
                // `CFPreferences` treats a path as a domain and then writes that file.
                if domain.is_empty()
                    || domain.contains(['/', '\0'])
                    || domain.starts_with(['.', '~'])
                {
                    return Err(LedgerError::InvalidTarget(domain.clone()));
                }
                if key.is_empty() || key.contains('\0') {
                    return Err(LedgerError::InvalidTarget(key.clone()));
                }
            }
            Self::Service { name, .. } => {
                if name.is_empty() || name.contains(['/', '\\', '\0']) {
                    return Err(LedgerError::InvalidTarget(name.clone()));
                }
            }
        }
        Ok(())
    }

    /// Whether this kind of target can hold `value`.
    fn accepts(&self, value: &Value) -> bool {
        match self {
            Self::Registry { .. } => match value {
                Value::Str(s) | Value::ExpandStr(s) => !s.contains('\0'),
                Value::MultiStr(items) => items.iter().all(|s| !s.is_empty() && !s.contains('\0')),
                Value::U32(_) | Value::U64(_) | Value::Bytes(_) | Value::Raw { .. } => true,
                _ => false,
            },
            Self::Preference { .. } => matches!(
                value,
                Value::Str(_) | Value::Bool(_) | Value::I64(_) | Value::Bytes(_) | Value::Plist(_)
            ),
            Self::Service { .. } => {
                matches!(value, Value::Program { path, .. } if !path.is_empty())
            }
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry { key, name } if name.is_empty() => write!(f, "{key} @ (default)"),
            Self::Registry { key, name } => write!(f, "{key} @ {name}"),
            Self::Preference { domain, key } => write!(f, "defaults {domain} {key}"),
            Self::Service { service, name } => write!(f, "{} {name}", service.name()),
        }
    }
}

impl Change {
    /// A change to a registry value under `HKEY_CURRENT_USER`. `key` is normalized to `HKCU\…`.
    pub fn registry(key: &str, name: &str, value: Option<Value>) -> Result<Self, LedgerError> {
        let subkey = hkcu_subkey(key)?;
        let change = Self {
            target: Target::Registry {
                key: format!("HKCU\\{subkey}"),
                name: name.to_owned(),
            },
            value,
        };
        change.validate()?;
        Ok(change)
    }

    /// A change to a macOS preference of the current user.
    pub fn preference(domain: &str, key: &str, value: Option<Value>) -> Result<Self, LedgerError> {
        let change = Self {
            target: Target::Preference {
                domain: domain.to_owned(),
                key: key.to_owned(),
            },
            value,
        };
        change.validate()?;
        Ok(change)
    }

    /// A change to a background service. Reserved for IDX-05: no backend handles it yet.
    pub fn service(
        service: ServiceKind,
        name: &str,
        value: Option<Value>,
    ) -> Result<Self, LedgerError> {
        let change = Self {
            target: Target::Service {
                service,
                name: name.to_owned(),
            },
            value,
        };
        change.validate()?;
        Ok(change)
    }

    /// Makes the app the program that opens folders for other apps on macOS (`NSFileViewer`).
    pub fn macos_file_viewer() -> Self {
        Self {
            target: Target::Preference {
                domain: GLOBAL_DOMAIN.to_owned(),
                key: FILE_VIEWER_KEY.to_owned(),
            },
            value: Some(Value::Str(rf_core::identity::BUNDLE_ID.to_owned())),
        }
    }

    /// Checks the target and the value against what this kind of target allows.
    ///
    /// Backends call this on every change they read back from the ledger file, so a tampered
    /// file cannot reach outside `HKEY_CURRENT_USER` or write to an arbitrary path.
    pub fn validate(&self) -> Result<(), LedgerError> {
        self.target.validate()?;
        match &self.value {
            Some(value) if !self.target.accepts(value) => Err(LedgerError::InvalidValue(format!(
                "{value:?} is not valid for {}",
                self.target
            ))),
            _ => Ok(()),
        }
    }
}
