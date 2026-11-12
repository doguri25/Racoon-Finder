//! `Vfs`, the table that routes a `VPath` to the provider of its scheme.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use rf_core::{Error, Result, Scheme, VPath};

use crate::local::LocalProvider;
use crate::provider::Provider;

/// The providers of the app, by scheme. Cloning is cheap and shares the providers.
#[derive(Clone, Default)]
pub struct Vfs {
    providers: BTreeMap<Scheme, Arc<dyn Provider>>,
}

impl Vfs {
    /// A registry with no providers.
    pub fn new() -> Vfs {
        Vfs::default()
    }

    /// A registry whose only provider is `LocalProvider`.
    pub fn with_local() -> Vfs {
        let mut vfs = Vfs::new();
        vfs.register(Arc::new(LocalProvider::new()));
        vfs
    }

    /// Serves the scheme of `provider` with it. Returns the provider it replaced, if any.
    pub fn register(&mut self, provider: Arc<dyn Provider>) -> Option<Arc<dyn Provider>> {
        self.providers.insert(provider.scheme(), provider)
    }

    /// The provider that serves `path`, or `Error::Unsupported` when none is registered for its
    /// scheme.
    pub fn provider(&self, path: &VPath) -> Result<Arc<dyn Provider>> {
        self.providers
            .get(&path.scheme())
            .cloned()
            .ok_or_else(|| Error::Unsupported(format!("no provider for {} paths", path.scheme())))
    }
}

impl fmt::Debug for Vfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vfs")
            .field("schemes", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}
