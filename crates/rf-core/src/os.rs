//! The operating system family a piece of host-independent logic should follow.

/// An operating system family.
///
/// Pure logic that must behave the same on every host (path layouts, name rules)
/// takes an `Os` argument instead of reading `cfg`, so every rule can be tested
/// on any machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    MacOs,
    /// Any other target; only used for development and CI (PRD 1.4).
    Other,
}

impl Os {
    /// The family of the target this build runs on.
    #[cfg(target_os = "windows")]
    pub const CURRENT: Self = Self::Windows;
    /// The family of the target this build runs on.
    #[cfg(target_os = "macos")]
    pub const CURRENT: Self = Self::MacOs;
    /// The family of the target this build runs on.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub const CURRENT: Self = Self::Other;
}
