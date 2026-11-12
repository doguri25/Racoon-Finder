use std::fmt;

/// Kind of location a `VPath` points into.
///
/// Deliberately not `non_exhaustive`: when a scheme is added, every `match` in the
/// workspace stops compiling until it handles the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scheme {
    Local,
    Archive,
    Remote,
    Shell,
    Collection,
    Search,
}

impl Scheme {
    /// Every scheme, in declaration order.
    pub const ALL: &[Scheme] = &[
        Scheme::Local,
        Scheme::Archive,
        Scheme::Remote,
        Scheme::Shell,
        Scheme::Collection,
        Scheme::Search,
    ];

    /// The lowercase name used in the string form (`local:...`).
    pub const fn name(self) -> &'static str {
        match self {
            Scheme::Local => "local",
            Scheme::Archive => "archive",
            Scheme::Remote => "remote",
            Scheme::Shell => "shell",
            Scheme::Collection => "collection",
            Scheme::Search => "search",
        }
    }

    /// Looks a scheme up by name, ignoring ASCII case.
    pub fn from_name(name: &str) -> Option<Scheme> {
        Scheme::ALL
            .iter()
            .copied()
            .find(|scheme| scheme.name().eq_ignore_ascii_case(name))
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.name())
    }
}
