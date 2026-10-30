//! The folder views remembered (NAV-10): at most [`MAX_FOLDER_VIEWS`], the one used longest ago
//! first, each folder known by its `VPath::to_uri` under the system's rule of names (case, and
//! on macOS the Unicode form). See the module `state::view` for what a view is and how a folder
//! finds its own.

use std::ffi::OsStr;

use rf_core::{Os, VPath};
use rf_vfs::{NameFolding, fold_name};

use super::FolderView;
use crate::pane::ancestors;

/// How many folders the store remembers a view for; past it, the one used longest ago goes.
pub const MAX_FOLDER_VIEWS: usize = 1000;
/// A folder whose `VPath::to_uri` is longer than this many bytes has no view remembered: a
/// thousand of them would make a settings file too large to read back.
pub const MAX_FOLDER_URI: usize = 2048;
/// The folder views remembered (NAV-10), the one used longest ago first.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewStore {
    /// With each the key it is found by ([`ViewStore::key`]).
    entries: Vec<(String, StoredView)>,
    /// Whose rule of names the keys follow.
    os: Os,
}

/// One folder's view in the [`ViewStore`].
#[derive(Clone, Debug, PartialEq)]
pub struct StoredView {
    /// The folder, as `VPath::to_uri` gave it (as the settings file has it).
    pub folder: String,
    pub view: FolderView,
    /// 하위 폴더에 적용: the folders below show this view too, unless they have one of their own.
    pub subfolders: bool,
    /// The values of a newer version that `view` cannot hold, written back as they are.
    pub unknown: UnknownParts,
    /// The keys of a newer version, written back as they are.
    pub extra: toml::Table,
}

/// The parts of a folder's view that a newer version wrote and this one does not know (a view
/// mode, a sort key, columns): the view shows its default in their place, and the settings
/// write them back as they were, so that the newer version finds them again. A part the user
/// changes here is this version's choice from then on, and the unknown value goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnknownParts {
    /// The view mode (`thumbnails`), in the place of 자세히.
    pub mode: Option<String>,
    /// The key that sorts first, in the place of the name (in the view's direction).
    pub sort: Option<String>,
    /// The second key, in the place of none, and whether it sorts descending.
    pub then: Option<(String, bool)>,
    /// The columns, with their widths (0 for none), after the columns of the view.
    pub columns: Vec<(String, u32)>,
}

impl UnknownParts {
    /// The view changed from `old` to `new`: the parts the change chose lose their unknown
    /// values. The unknown columns stay, whatever the known ones do.
    fn changed(&mut self, old: &FolderView, new: &FolderView) {
        if new.mode != old.mode {
            self.mode = None;
        }
        if new.sort.primary != old.sort.primary {
            self.sort = None;
        }
        if new.sort.secondary != old.sort.secondary {
            self.then = None;
        }
    }
}

impl Default for ViewStore {
    fn default() -> ViewStore {
        ViewStore::new(Vec::new(), Os::CURRENT)
    }
}

impl ViewStore {
    /// A store of `entries`, the one used longest ago first, with the rule of names of `os`. Of two
    /// entries for one folder the later one is kept, and of more than [`MAX_FOLDER_VIEWS`] the
    /// last ones.
    pub fn new(entries: Vec<StoredView>, os: Os) -> ViewStore {
        let mut store = ViewStore {
            entries: Vec::with_capacity(entries.len().min(MAX_FOLDER_VIEWS)),
            os,
        };
        for entry in entries {
            let key = store.key_of(&entry.folder);
            store.entries.retain(|(known, _)| *known != key);
            store.entries.push((key, entry));
        }
        store.trim();
        store
    }

    /// The folders' views, the one used longest ago first.
    pub fn entries(&self) -> impl Iterator<Item = &StoredView> {
        self.entries.iter().map(|(_, entry)| entry)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The view of `folder`, as its own, or the one it takes from above, or the default one
    /// (see the module's documentation). The entry it comes from counts as used.
    pub fn view_for(&mut self, folder: &VPath) -> FolderView {
        if let Some(at) = self.position(&self.key(folder)) {
            return self.touch(at).view.clone();
        }
        // The nearest folder above that applies its view to its subfolders.
        let mut above = ancestors(folder);
        above.pop();
        let found = above.iter().rev().find_map(|parent| {
            let at = self.position(&self.key(parent))?;
            self.entries[at].1.subfolders.then_some(at)
        });
        match found {
            Some(at) => self.touch(at).view.clone(),
            None => FolderView::default(),
        }
    }

    /// The view `folder` has of its own, if it has one.
    pub fn own(&self, folder: &VPath) -> Option<&StoredView> {
        self.position(&self.key(folder))
            .map(|at| &self.entries[at].1)
    }

    /// Remembers `view` as the view of `folder`, which keeps whether it applies to its
    /// subfolders; it is the one used last. Past [`MAX_FOLDER_VIEWS`] the one used longest ago
    /// goes. Nothing for a folder whose `to_uri` is longer than [`MAX_FOLDER_URI`].
    pub fn remember(&mut self, folder: &VPath, view: FolderView) {
        let uri = folder.to_uri();
        if uri.len() > MAX_FOLDER_URI {
            return;
        }
        let key = self.key_of(&uri);
        if let Some(at) = self.position(&key) {
            let entry = self.touch(at);
            entry.unknown.changed(&entry.view, &view);
            entry.view = view;
        } else {
            self.entries.push((
                key,
                StoredView {
                    folder: uri,
                    view,
                    subfolders: false,
                    unknown: UnknownParts::default(),
                    extra: toml::Table::new(),
                },
            ));
            self.trim();
        }
    }

    /// Whether a view of `folder` can be remembered: its `to_uri` is no longer than
    /// [`MAX_FOLDER_URI`].
    pub fn keeps(&self, folder: &VPath) -> bool {
        folder.to_uri().len() <= MAX_FOLDER_URI
    }

    /// Whether `a` and `b` are one folder, as the store tells folders apart (ignoring case on
    /// Windows and macOS, and the Unicode form on macOS).
    pub fn same_folder(&self, a: &VPath, b: &VPath) -> bool {
        self.key(a) == self.key(b)
    }

    /// How many folders below `folder` have a view of their own: those 하위 폴더에 적용 would
    /// forget.
    pub fn count_below(&self, folder: &VPath) -> usize {
        let below = self.below(folder);
        self.entries
            .iter()
            .filter(|(known, _)| below(known))
            .count()
    }

    /// 하위 폴더에 적용: `view` is the view of `folder` and of every folder below it. The folders
    /// below forget their own views, so that they show this one, now and when they are made;
    /// one that is changed later has its own again. `false`, and no change, for a folder whose
    /// view is not remembered ([`MAX_FOLDER_URI`]).
    pub fn apply_to_subfolders(&mut self, folder: &VPath, view: FolderView) -> bool {
        self.remember(folder, view);
        let Some(at) = self.position(&self.key(folder)) else {
            return false;
        };
        self.entries[at].1.subfolders = true;
        let below = self.below(folder);
        self.entries.retain(|(known, _)| !below(known));
        true
    }

    /// Whether the folder of a key is below `folder` (not `folder` itself).
    fn below(&self, folder: &VPath) -> impl Fn(&str) -> bool + use<> {
        let base = VPath::parse(&self.key(folder)).ok();
        move |known| {
            base.as_ref()
                .zip(VPath::parse(known).ok())
                .is_some_and(|(base, path)| path != *base && path.starts_with(base))
        }
    }

    /// 기본값으로 되돌리기: `folder` forgets its own view and whether it applied it to its
    /// subfolders; it shows what it takes from above, or the default view. The folders below
    /// keep their own views. `false` when it had none.
    pub fn reset(&mut self, folder: &VPath) -> bool {
        let Some(at) = self.position(&self.key(folder)) else {
            return false;
        };
        self.entries.remove(at);
        true
    }

    /// The key `folder` is found by: its `to_uri`, folded by the system's rule of names
    /// ([`rf_vfs::fold_name`]: lower-cased on Windows and macOS, composed to NFC on macOS, as it
    /// is elsewhere). The `to_uri` keeps every letter as it is, escaping only `%` and control
    /// characters, so the folding sees the names.
    fn key(&self, folder: &VPath) -> String {
        self.key_of(&folder.to_uri())
    }

    fn key_of(&self, uri: &str) -> String {
        fold_name(OsStr::new(uri), NameFolding::of(self.os))
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|(known, _)| known == key)
    }

    /// Makes the entry at `at` the one used last, and gives it.
    fn touch(&mut self, at: usize) -> &mut StoredView {
        let entry = self.entries.remove(at);
        self.entries.push(entry);
        &mut self.entries.last_mut().expect("just pushed").1
    }

    /// Drops the entries used longest ago past [`MAX_FOLDER_VIEWS`].
    fn trim(&mut self) {
        let over = self.entries.len().saturating_sub(MAX_FOLDER_VIEWS);
        self.entries.drain(..over);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ViewMode;

    fn folder(path: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(path)).expect("a local path")
    }

    fn icons(mode: ViewMode) -> FolderView {
        FolderView {
            mode,
            ..FolderView::default()
        }
    }

    #[test]
    fn a_folder_takes_its_own_view_then_one_applied_from_above_then_the_default() {
        let mut store = ViewStore::new(Vec::new(), Os::Other);
        let (top, mid, low) = (folder("top"), folder("top/mid"), folder("top/mid/low"));
        assert_eq!(store.view_for(&low), FolderView::default());

        store.remember(&top, icons(ViewMode::LargeIcons));
        // Remembered for `top` alone: the folders below do not take it.
        assert_eq!(store.view_for(&mid), FolderView::default());
        assert_eq!(store.view_for(&top), icons(ViewMode::LargeIcons));

        // Applied to the subfolders: every folder below shows it, but one with a view of its own.
        store.remember(&mid, icons(ViewMode::List));
        store.remember(&low, icons(ViewMode::List));
        assert_eq!(store.count_below(&top), 2, "mid and low, not top itself");
        assert_eq!(store.count_below(&mid), 1);
        assert_eq!(store.count_below(&low), 0);
        assert!(store.apply_to_subfolders(&top, icons(ViewMode::SmallIcons)));
        assert_eq!(store.count_below(&top), 0);
        assert_eq!(store.view_for(&low), icons(ViewMode::SmallIcons));
        assert_eq!(
            store.view_for(&mid),
            icons(ViewMode::SmallIcons),
            "forgot its own"
        );
        assert!(store.own(&mid).is_none());
        store.remember(&mid, icons(ViewMode::List));
        assert_eq!(store.view_for(&mid), icons(ViewMode::List));
        assert_eq!(store.view_for(&low), icons(ViewMode::SmallIcons));
        // A change of `top` keeps applying below.
        store.remember(&top, icons(ViewMode::MediumIcons));
        assert_eq!(store.view_for(&low), icons(ViewMode::MediumIcons));

        // Reset: `top` forgets its own view and its applying below; `mid` keeps its own.
        assert!(store.reset(&top));
        assert!(!store.reset(&top));
        assert_eq!(store.view_for(&top), FolderView::default());
        assert_eq!(store.view_for(&low), FolderView::default());
        assert_eq!(store.view_for(&mid), icons(ViewMode::List));
        // Reset below a folder that applies its view: it takes that one again.
        assert!(store.apply_to_subfolders(&top, icons(ViewMode::LargeIcons)));
        store.remember(&low, icons(ViewMode::List));
        assert!(store.reset(&low));
        assert_eq!(store.view_for(&low), icons(ViewMode::LargeIcons));
    }

    #[test]
    fn folders_are_known_by_the_systems_rule_of_case() {
        let (upper, lower) = (folder("Photos"), folder("photos"));
        for (os, alike) in [(Os::Windows, true), (Os::MacOs, true), (Os::Other, false)] {
            let mut store = ViewStore::new(Vec::new(), os);
            store.remember(&upper, icons(ViewMode::List));
            assert_eq!(
                store.view_for(&lower) == icons(ViewMode::List),
                alike,
                "{os:?}"
            );
            // One entry either way when the same folder is remembered under both spellings.
            store.remember(&lower, icons(ViewMode::SmallIcons));
            assert_eq!(store.len(), if alike { 1 } else { 2 }, "{os:?}");
            assert_eq!(store.same_folder(&upper, &lower), alike, "{os:?}");
            // The folders below, too.
            let mut store = ViewStore::new(Vec::new(), os);
            store.apply_to_subfolders(&upper, icons(ViewMode::LargeIcons));
            let below = folder("photos/2026");
            assert_eq!(
                store.view_for(&below) == icons(ViewMode::LargeIcons),
                alike,
                "{os:?}"
            );
        }
    }

    #[test]
    fn folders_are_known_by_the_systems_rule_of_unicode_form() {
        // 사진 composed (NFC) and decomposed (NFD): one folder on macOS, two elsewhere.
        let composed = folder("사진");
        let decomposed = folder("\u{1109}\u{1161}\u{110c}\u{1175}\u{11ab}");
        assert_ne!(composed, decomposed);
        for (os, alike) in [(Os::MacOs, true), (Os::Windows, false), (Os::Other, false)] {
            let mut store = ViewStore::new(Vec::new(), os);
            store.remember(&decomposed, icons(ViewMode::List));
            assert_eq!(
                store.view_for(&composed) == icons(ViewMode::List),
                alike,
                "{os:?}"
            );
            assert_eq!(store.same_folder(&composed, &decomposed), alike, "{os:?}");
            // The folders below, too.
            let mut store = ViewStore::new(Vec::new(), os);
            store.apply_to_subfolders(&decomposed, icons(ViewMode::LargeIcons));
            let below = composed.join("2026").expect("a name");
            assert_eq!(
                store.view_for(&below) == icons(ViewMode::LargeIcons),
                alike,
                "{os:?}"
            );
        }
    }

    #[test]
    fn the_store_drops_the_view_used_longest_ago() {
        let mut store = ViewStore::new(Vec::new(), Os::Other);
        let numbered = |i: usize| folder(&format!("f{i}"));
        for i in 0..MAX_FOLDER_VIEWS {
            store.remember(&numbered(i), icons(ViewMode::List));
        }
        assert_eq!(store.len(), MAX_FOLDER_VIEWS);
        // Showing f0 uses it: f1 is the one used longest ago now, and goes first.
        assert_eq!(store.view_for(&numbered(0)), icons(ViewMode::List));
        store.remember(&numbered(MAX_FOLDER_VIEWS), icons(ViewMode::List));
        assert_eq!(store.len(), MAX_FOLDER_VIEWS);
        assert!(store.own(&numbered(0)).is_some());
        assert!(store.own(&numbered(1)).is_none());
        assert!(store.own(&numbered(2)).is_some());

        // A folder too long to keep is not remembered.
        let long = folder(&"x".repeat(MAX_FOLDER_URI));
        store.remember(&long, icons(ViewMode::List));
        assert!(store.own(&long).is_none());
        assert!(!store.apply_to_subfolders(&long, icons(ViewMode::List)));

        // A store read from more entries than it keeps keeps the last ones, and of two for one
        // folder the later.
        let stored = |i: usize, mode| StoredView {
            folder: numbered(i).to_uri(),
            view: icons(mode),
            subfolders: false,
            unknown: UnknownParts::default(),
            extra: toml::Table::new(),
        };
        let mut many: Vec<StoredView> = (0..=MAX_FOLDER_VIEWS)
            .map(|i| stored(i, ViewMode::List))
            .collect();
        many.push(stored(5, ViewMode::LargeIcons));
        let read = ViewStore::new(many, Os::Other);
        assert_eq!(read.len(), MAX_FOLDER_VIEWS);
        // 1001 folders, f5 the one used last: f0 goes.
        assert!(read.own(&numbered(0)).is_none());
        assert!(read.own(&numbered(1)).is_some());
        assert_eq!(
            read.own(&numbered(5)).map(|entry| entry.view.mode),
            Some(ViewMode::LargeIcons)
        );
        assert_eq!(
            read.entries().last().map(|entry| entry.folder.clone()),
            Some(numbered(5).to_uri())
        );
    }

    #[test]
    fn a_change_here_drops_the_unknown_value_of_what_it_changed() {
        let mut store = ViewStore::new(
            vec![StoredView {
                folder: folder("new").to_uri(),
                view: FolderView::default(),
                subfolders: false,
                unknown: UnknownParts {
                    mode: Some("thumbnails".to_owned()),
                    sort: Some("rating".to_owned()),
                    then: Some(("tags".to_owned(), true)),
                    columns: vec![("tags".to_owned(), 80)],
                },
                extra: toml::Table::new(),
            }],
            Os::Other,
        );
        let unknown = |store: &ViewStore| store.own(&folder("new")).expect("kept").unknown.clone();
        // A change of the columns keeps every unknown part.
        let mut view = FolderView::default();
        view.toggle_column(crate::state::Column::Kind);
        store.remember(&folder("new"), view.clone());
        assert_eq!(unknown(&store).mode.as_deref(), Some("thumbnails"));
        assert_eq!(unknown(&store).columns, [("tags".to_owned(), 80)]);
        // The mode chosen here is this version's; the order stays unknown until it changes.
        view.mode = ViewMode::List;
        store.remember(&folder("new"), view.clone());
        assert_eq!(unknown(&store).mode, None);
        assert_eq!(unknown(&store).sort.as_deref(), Some("rating"));
        view.sort_by(rf_vfs::SortKey::Size, false);
        store.remember(&folder("new"), view);
        assert_eq!(unknown(&store).sort, None);
        assert_eq!(unknown(&store).then, Some(("tags".to_owned(), true)));
        assert_eq!(unknown(&store).columns.len(), 1, "the unknown columns stay");
    }
}
