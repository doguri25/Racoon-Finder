//! Filtering of directory entries by visibility.

use rf_core::{Attributes, Entry};

/// Which hidden and system items a listing shows, with the rules of Windows Explorer's
/// "Hidden items" and "Hide protected operating system files" options.
///
/// The default shows neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VisibilityFilter {
    pub show_hidden: bool,
    pub show_system: bool,
}

impl VisibilityFilter {
    /// A protected item (hidden and system at once) shows only when both options are on, a
    /// hidden item when `show_hidden` is on, and everything else always. A system item that is
    /// not hidden is not protected, so it always shows.
    pub fn is_visible(&self, entry: &Entry) -> bool {
        let hidden = entry.attributes.contains(Attributes::HIDDEN);
        let system = entry.attributes.contains(Attributes::SYSTEM);
        match (hidden, system) {
            (true, true) => self.show_hidden && self.show_system,
            (true, false) => self.show_hidden,
            (false, _) => true,
        }
    }

    /// Keeps only the entries that are visible, in their order.
    pub fn retain(&self, entries: &mut Vec<Entry>) {
        entries.retain(|entry| self.is_visible(entry));
    }
}
