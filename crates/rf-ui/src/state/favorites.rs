//! The sidebar's favorites the user keeps (NAV-05, DND-01) and its group of recent folders. A
//! child of [`state`](super), so these methods see the fields of [`AppState`].
//!
//! **Favorites.** The sidebar lists the system's well-known folders under "즐겨찾기" until the
//! user first edits the list; the first edit (an addition, a removal, a new name, another order)
//! makes the list they see the user's own ([`Settings::favorites`](crate::settings::Settings)),
//! which the settings keep from then on, and the folders the system adds later do not join it.
//! A list the user emptied stays empty ([`AppState::favorites_edited`]). The favorites are the
//! first roots of the folder tree, so a change of the list starts the tree again from the new
//! roots ([`AppState::set_places`]).
//!
//! **Drops.** A drag of folders over the gaps between the favorites, or the empty part of their
//! list ([`DropSpot::Favorites`]), is an addition, never a move or a copy: nothing is asked of
//! the job engine, the other app is told that nothing was done, and a folder that is among the
//! favorites already goes to the place of the drop rather than into the list twice. The list
//! takes folders only: a drag that holds a file, or a link key, is refused
//! ([`Unkept`]). Of another app's drag nothing is looked at while it hovers (a path tells nothing
//! of it, and the UI thread never touches the disk): it is answered as kept, and at the drop a
//! worker keeps the folders among the paths ([`Effect::KeepDropped`], [`AppState::folders_kept`]),
//! with a notice when some were not. A drop onto the row of a favorite is the existing rule: the items go into its
//! folder ([`DropSpot::Tree`]).
//!
//! **Recent folders.** The last [`RECENT_FOLDERS`] folders either pane showed, the latest first
//! ([`AppState::note_visits`], which the controller calls after every change, as it does
//! [`AppState::follow_active`]), listed in a group under the favorites that the user can fold. A
//! worker looks which are still there ([`Effect::CheckFolders`], at the start and again every half
//! minute); a folder that is gone dims, favorite or recent, and a click on it says so.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use rf_core::VPath;
use rf_platform::dnd::DragMods;

use super::drop_rule;
use super::{AppState, Effect, Hover, load};
use crate::dialogs::{Modal, NameDialog};
use crate::format::folder_label;
use crate::places::{Place, PlaceKind, recent_slot};
use crate::settings::{Favorite, Settings};
use crate::words;

/// How many folders the group of recent folders keeps.
pub const RECENT_FOLDERS: usize = 10;

/// How long a look at whether the favorites and the recent folders are still there is good for:
/// the next one is asked for no sooner ([`AppState::check_folders_due`]).
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// Why the list of favorites takes nothing of a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unkept {
    /// The drag holds a file (or something else than a folder): the list takes folders only.
    Files,
    /// The keys ask for a link: the list takes folders only as additions, and makes no links.
    Link,
}

/// Where the keyboard cursor is in the group of recent folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecentAt {
    /// On the head "최근 폴더", which folds the group.
    Head,
    /// On the folder `n` of the group.
    Item(usize),
}

/// What [`AppState`] keeps of the favorites and the recent folders.
#[derive(Clone, Debug, Default)]
pub(super) struct Kept {
    /// The favorites the user keeps; meaningful once `edited`.
    pub(super) favorites: Vec<Favorite>,
    /// The user edited the list: it is theirs, however short.
    pub(super) edited: bool,
    /// The system's well-known folders as the places worker found them: what the list shows
    /// until it is edited, and what the first edit starts from.
    pub(super) known: Vec<Place>,
    /// The folders visited last, the latest first.
    pub(super) recent: Vec<VPath>,
    /// The folders of `recent` and of the favorites a worker found missing.
    pub(super) missing: HashSet<VPath>,
    /// Counts the times the favorites were set from outside (the settings): what a worker was
    /// asked before is none of the new list's ([`AppState::folders_kept`]).
    pub(super) generation: u64,
    /// When the last look at the folders was asked for.
    pub(super) checked: Option<Instant>,
    /// Whether a worker is looking at the folders now (one at a time: a share that does not
    /// answer holds it for long, and no second look piles up behind it).
    pub(super) look: Look,
    /// The group is folded to its head.
    pub(super) folded: bool,
    /// The folder each pane showed when [`AppState::note_visits`] looked last.
    pub(super) visited: [Option<VPath>; 2],
    /// The keyboard cursor, while it is in the group (and then on no row of the tree).
    pub(super) cursor: Option<RecentAt>,
}

/// How the look at the folders stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Look {
    /// No worker looks.
    #[default]
    Idle,
    /// A worker looks.
    Running,
    /// A worker looks, and another look was asked for meanwhile: it follows the answer.
    Queued,
}

impl Look {
    /// Whether a look may start now: not while one is under way (it is noted that another is
    /// wanted). Marks the look as under way when it may.
    fn may_start(&mut self) -> bool {
        if *self == Look::Idle {
            *self = Look::Running;
            return true;
        }
        *self = Look::Queued;
        false
    }

    /// The worker answered; whether a look was asked for meanwhile.
    fn answered(&mut self) -> bool {
        std::mem::take(self) == Look::Queued
    }
}

impl AppState {
    /// The places of the sidebar, in order: the favorites, then the drives.
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// The favorites the user keeps, in order. Empty while the user has not edited the list
    /// ([`AppState::favorites_edited`]), and the sidebar then shows the system's folders.
    pub fn favorites(&self) -> &[Favorite] {
        &self.kept.favorites
    }

    /// Whether the user edited the list of favorites, even to empty it: it is theirs then, and
    /// the settings keep it.
    pub fn favorites_edited(&self) -> bool {
        self.kept.edited
    }

    /// The favorites the settings kept; `emptied` for a list the user emptied. A favorite
    /// whose folder this version cannot read is left out.
    pub fn set_favorites(&mut self, favorites: Vec<Favorite>, emptied: bool) -> Vec<Effect> {
        let favorites: Vec<Favorite> = favorites
            .into_iter()
            .filter(|favorite| favorite.folder().is_some())
            .collect();
        self.kept.edited = emptied || !favorites.is_empty();
        self.kept.favorites = favorites;
        self.kept.generation += 1;
        if self.places.is_empty() {
            return Vec::new();
        }
        self.favorites_changed()
    }

    /// Takes the favorites, the recent folders and whether their group is folded from the
    /// settings the app started with; a worker looks which recent folders are still there.
    pub fn restore_sidebar(&mut self, settings: &Settings) -> Vec<Effect> {
        let mut effects =
            self.set_favorites(settings.favorites.clone(), settings.favorites_emptied);
        effects.extend(self.set_recent_folders(settings.recent_visits()));
        self.set_recent_folded(settings.recent_folded);
        effects
    }

    /// Puts what the settings keep of the sidebar into `settings`: the favorites the user keeps
    /// (none until they edited the list, or a list they emptied), the recent folders and whether
    /// their group is folded.
    pub fn save_sidebar(&self, settings: &mut Settings) {
        settings.favorites.clone_from(&self.kept.favorites);
        settings.favorites_emptied = self.kept.edited && settings.favorites.is_empty();
        settings.set_recent_visits(&self.kept.recent);
        settings.recent_folded = self.kept.folded;
    }

    /// The places the list of favorites shows: the user's, or the system's until the first
    /// edit.
    pub(super) fn shown_favorites(&self) -> Vec<Place> {
        if !self.kept.edited {
            return self.kept.known.clone();
        }
        self.kept
            .favorites
            .iter()
            .filter_map(|favorite| {
                Some(Place {
                    label: favorite.name.clone(),
                    path: favorite.folder()?,
                    kind: PlaceKind::Kept,
                    space: None,
                })
            })
            .collect()
    }

    /// The favorites as they show now, as the user's list would hold them.
    fn shown_as_list(&self) -> Vec<Favorite> {
        if self.kept.edited {
            self.kept.favorites.clone()
        } else {
            self.kept
                .known
                .iter()
                .map(|place| Favorite::new(place.label.clone(), &place.path))
                .collect()
        }
    }

    /// Makes `list` the user's favorites, if it differs from what shows, and starts the tree
    /// again from the new roots. When the tree has the keyboard cursor, it stays with the
    /// favorite `focus` of the new list.
    fn keep(&mut self, list: Vec<Favorite>, focus: Option<usize>) -> Vec<Effect> {
        let same = if self.kept.edited {
            list == self.kept.favorites
        } else {
            list == self.shown_as_list()
        };
        if same {
            return Vec::new();
        }
        let cursor = self.tree_cursor.is_some();
        let focus = focus.map(|index| index.min(list.len().saturating_sub(1)));
        self.kept.edited = true;
        self.kept.favorites = list;
        let mut effects = self.favorites_changed();
        if cursor
            && let Some(focus) = focus
            && let Some(row) = self.tree.rows().iter().position(|row| {
                row.root.is_some() && self.favorite_of_row_index(row.root) == Some(focus)
            })
        {
            self.tree_cursor = self.tree.node_at(row);
            effects.push(Effect::ScrollTree(row));
        }
        effects
    }

    /// The favorites changed: the sidebar's places and the tree's roots follow.
    fn favorites_changed(&mut self) -> Vec<Effect> {
        let rest: Vec<Place> = self
            .places
            .drain(..)
            .filter(|place| !place.kind.is_favorite())
            .collect();
        self.places = self.shown_favorites().into_iter().chain(rest).collect();
        self.reroot_places()
    }

    /// Starts the sidebar's tree again from the places as roots, and the picker's when one is
    /// open.
    pub(super) fn reroot_places(&mut self) -> Vec<Effect> {
        // The new tree's nodes are other ones.
        self.tree_cursor = None;
        if matches!(self.kept.cursor, Some(RecentAt::Item(_))) {
            self.kept.cursor = Some(RecentAt::Head);
        }
        let roots = super::pick::tree_roots(&self.places);
        let mut effects = self.with_tree(|tree| tree.set_roots(roots.clone()));
        if self.shown_picker().is_some() {
            effects.extend(self.with_pick_tree(|tree| tree.set_roots(roots)));
        }
        effects
    }

    /// Keeps the folder `folder` as the last favorite (the menu's 현재 폴더를 즐겨찾기에 추가, a
    /// folder's 즐겨찾기에 추가). One that is among the favorites already says so.
    pub fn favorite_add(&mut self, folder: &VPath) -> Vec<Effect> {
        let list = self.shown_as_list();
        if let Some(there) = list
            .iter()
            .find(|favorite| favorite.folder().as_ref() == Some(folder))
        {
            return vec![Effect::Notice(words::favorite_already(&there.name))];
        }
        let (list, _) = put(list, vec![folder.clone()], None);
        self.keep(list, None)
    }

    /// Takes favorite `index` out of the list; nothing asks first (to put it back, add it
    /// again).
    pub fn favorite_remove(&mut self, index: usize) -> Vec<Effect> {
        let mut list = self.shown_as_list();
        if index >= list.len() {
            return Vec::new();
        }
        list.remove(index);
        self.keep(list, Some(index))
    }

    /// Shows favorite `index` under `name` (without the spaces around it); a name of nothing
    /// changes nothing.
    pub fn favorite_rename(&mut self, index: usize, name: &str) -> Vec<Effect> {
        let name = name.trim();
        let mut list = self.shown_as_list();
        if name.is_empty() || index >= list.len() {
            return Vec::new();
        }
        name.clone_into(&mut list[index].name);
        self.keep(list, Some(index))
    }

    /// Moves favorite `from` so that it is favorite `to` of the list afterwards (the ends stop
    /// it).
    pub fn favorite_move(&mut self, from: usize, to: usize) -> Vec<Effect> {
        let mut list = self.shown_as_list();
        if from >= list.len() {
            return Vec::new();
        }
        let item = list.remove(from);
        let to = to.min(list.len());
        list.insert(to, item);
        self.keep(list, Some(to))
    }

    /// An item of the menu of favorite `index` (its row's menu, or the menu key on it): `open`,
    /// `open-other`, `rename`, `remove`, `up`, `down`.
    pub fn favorite_command(&mut self, index: usize, name: &str) -> Vec<Effect> {
        let list = self.shown_as_list();
        let Some(favorite) = list.get(index) else {
            return Vec::new();
        };
        let Some(folder) = favorite.folder() else {
            return Vec::new();
        };
        match name {
            "open" if self.kept.missing.contains(&folder) => {
                vec![Effect::Notice(words::start_folder_gone(&folder))]
            }
            "open" => self.go_to(folder),
            "open-other" => {
                let other = 1 - self.active;
                load(self.navigate_shown(other, folder))
            }
            "rename" => {
                self.open(Modal::Name(NameDialog::favorite(index, &favorite.name)));
                Vec::new()
            }
            "remove" => self.favorite_remove(index),
            "up" if index > 0 => self.favorite_move(index, index - 1),
            "down" => self.favorite_move(index, index + 1),
            _ => Vec::new(),
        }
    }

    /// 즐겨찾기에 추가 in the menu of the folder item of the active pane: the folders of the
    /// items the menu is of are kept as favorites (one that is a favorite already says so).
    pub fn favorite_add_selected(&mut self) -> Vec<Effect> {
        let folders: Vec<VPath> = self
            .target_entries(self.active)
            .into_iter()
            .filter(|(_, entry)| entry.kind == rf_core::EntryKind::Dir)
            .map(|(path, _)| path)
            .collect();
        match folders.as_slice() {
            [] => Vec::new(),
            [one] => self.favorite_add(one),
            _ => self.keep_folders(folders, None),
        }
    }

    /// 이 폴더를 즐겨찾기에 추가 in the menu of a row of the sidebar that is no favorite: the
    /// folder of row `row` of the tree joins the favorites.
    pub fn favorite_add_tree_row(&mut self, row: usize) -> Vec<Effect> {
        let folder = self
            .tree
            .node_at(row)
            .and_then(|id| self.tree.node(id))
            .map(|node| node.path.clone());
        folder.map_or_else(Vec::new, |folder| self.favorite_add(&folder))
    }

    /// The same for recent folder `index`.
    pub fn favorite_add_recent(&mut self, index: usize) -> Vec<Effect> {
        let folder = self.kept.recent.get(index).cloned();
        folder.map_or_else(Vec::new, |folder| self.favorite_add(&folder))
    }

    /// An item of the menu of the sidebar's empty part: `add-current` keeps the folder the
    /// active pane shows.
    pub fn favorites_blank_command(&mut self, name: &str) -> Vec<Effect> {
        match name {
            "add-current" => {
                let folder = self.tab(self.active).location().clone();
                self.favorite_add(&folder)
            }
            _ => Vec::new(),
        }
    }

    /// The folder of favorite `index` as the settings keep it, for the payload of a drag of it.
    pub fn favorite_folder_text(&self, index: usize) -> Option<String> {
        self.shown_as_list()
            .get(index)
            .map(|favorite| favorite.path.clone())
    }

    /// Where favorite `from` (whose folder is `folder`, as the payload of its drag says, so that
    /// a list that changed since the press is not mistaken) would be after a drop in the gap
    /// `gap` of the list (`None`: at the end); `None` where that is where it is, or the list is
    /// not what the drag started from.
    pub fn favorite_reorder_to(
        &self,
        from: usize,
        folder: &str,
        gap: Option<usize>,
    ) -> Option<usize> {
        let list = self.shown_as_list();
        if list.get(from)?.path != folder {
            return None;
        }
        let gap = gap.map_or(list.len(), |gap| gap.min(list.len()));
        let to = if gap > from { gap - 1 } else { gap };
        (to != from).then_some(to)
    }

    /// The favorite a row of the folder tree is, as its place's position among the favorites;
    /// `None` for a folder below a place, and for a drive.
    pub(super) fn favorite_of_row(&self, row: usize) -> Option<usize> {
        self.favorite_of_row_index(self.tree.rows().get(row)?.root)
    }

    /// The favorite a root place of the tree is (its position among the roots), if it is one.
    fn favorite_of_row_index(&self, root: Option<usize>) -> Option<usize> {
        let root = root?;
        let place = self.places.get(root)?;
        place.kind.is_favorite().then(|| {
            self.places[..root]
                .iter()
                .filter(|place| place.kind.is_favorite())
                .count()
        })
    }

    // ----------------------------------------------------------------------------------------
    // Drops on the list of favorites.

    /// What the gap `index` of the list of favorites (`None`: its end) makes of the drag of rows
    /// whose payload carries `id`, with the keys `mods` held.
    pub(super) fn keep_hover_rows(
        &mut self,
        id: u64,
        index: Option<usize>,
        mods: DragMods,
    ) -> Hover {
        if self.modal.is_some() || self.quitting {
            return Hover::Nothing;
        }
        let Some(drag) = self.drag.as_mut().filter(|drag| drag.id == id && id != 0) else {
            return Hover::Nothing;
        };
        // Nothing here opens by a rest.
        drag.shown = None;
        let count = drag.request.sources.len();
        let only_folders = drag.request.folder_count == count;
        Self::keep_verdict(only_folders, count, index, mods)
    }

    /// What a drop of `count` items on the gap `index` would do, `only_folders` or not.
    pub(super) fn keep_verdict(
        only_folders: bool,
        count: usize,
        index: Option<usize>,
        mods: DragMods,
    ) -> Hover {
        if drop_rule::asks_for_link(rf_core::Os::CURRENT, mods) {
            Hover::NoKeep(Unkept::Link)
        } else if !only_folders || count == 0 {
            Hover::NoKeep(Unkept::Files)
        } else {
            Hover::Keeps { index, count }
        }
    }

    /// The drag of rows whose payload carries `id` was dropped on the gap `index` of the list
    /// of favorites: its folders are kept there, and nothing moves. The drag is over.
    pub(super) fn keep_dropped_rows(
        &mut self,
        id: u64,
        index: Option<usize>,
        mods: DragMods,
    ) -> Vec<Effect> {
        if !matches!(self.keep_hover_rows(id, index, mods), Hover::Keeps { .. }) {
            return Vec::new();
        }
        let Some(drag) = self.drag.take() else {
            return Vec::new();
        };
        self.pending_click = None;
        self.keep_folders(drag.request.sources, index)
    }

    /// A worker looked at the paths of another app's drag that were dropped on the gap `index` of
    /// the list ([`Effect::KeepDropped`]): the `folders` among them are kept there, and a notice
    /// tells when some were not folders (`others`). A list set from the settings since (a
    /// different `generation`) is not the one that was dropped on.
    pub fn folders_kept(
        &mut self,
        generation: u64,
        folders: Vec<VPath>,
        others: bool,
        index: Option<usize>,
    ) -> Vec<Effect> {
        if generation != self.kept.generation {
            return Vec::new();
        }
        let mut effects = if folders.is_empty() {
            Vec::new()
        } else {
            self.keep_folders(folders, index)
        };
        if others {
            effects.push(Effect::Notice(words::drop_only_folders().to_owned()));
        }
        effects
    }

    /// Keeps `folders` as favorites from the gap `index` on (`None`: at the end), in order; one
    /// that is a favorite already goes to its place there.
    pub(super) fn keep_folders(
        &mut self,
        folders: Vec<VPath>,
        index: Option<usize>,
    ) -> Vec<Effect> {
        let (list, _) = put(self.shown_as_list(), folders, index);
        self.keep(list, None)
    }

    // ----------------------------------------------------------------------------------------
    // Recent folders.

    /// The folders visited last, the latest first (at most [`RECENT_FOLDERS`]).
    pub fn recent_folders(&self) -> &[VPath] {
        &self.kept.recent
    }

    /// The folders the settings kept as the recent ones; a worker looks which are still there
    /// ([`Effect::CheckFolders`]).
    pub fn set_recent_folders(&mut self, folders: Vec<VPath>) -> Vec<Effect> {
        let mut seen = HashSet::new();
        self.kept.recent = folders
            .into_iter()
            .filter(|folder| seen.insert(folder.clone()))
            .take(RECENT_FOLDERS)
            .collect();
        self.kept.missing.clear();
        self.kept.cursor = None;
        self.look_at_folders()
    }

    /// The folders a worker is asked about: the recent ones, then the favorites as they show.
    fn folders_to_check(&self) -> Vec<VPath> {
        let mut seen = HashSet::new();
        self.kept
            .recent
            .iter()
            .cloned()
            .chain(self.shown_as_list().iter().filter_map(Favorite::folder))
            .filter(|folder| seen.insert(folder.clone()))
            .collect()
    }

    /// Asks a worker which of the recent folders and favorites are still there; nothing while
    /// there are none.
    fn look_at_folders(&mut self) -> Vec<Effect> {
        let folders = self.folders_to_check();
        if folders.is_empty() {
            return Vec::new();
        }
        if !self.kept.look.may_start() {
            return Vec::new();
        }
        vec![Effect::CheckFolders(folders)]
    }

    /// The same look, if none was asked for within the last half minute: the controller calls
    /// this after every change, so the favorites and recent folders that vanish while the app
    /// runs (a drive taken away, a folder deleted) dim by the time the sidebar is looked at
    /// again, at a cost of a few `stat`s on a worker thread, never more than twice a minute.
    pub fn check_folders_due(&mut self, now: Instant) -> Vec<Effect> {
        if self
            .kept
            .checked
            .is_some_and(|before| now.saturating_duration_since(before) < CHECK_EVERY)
        {
            return Vec::new();
        }
        let effects = self.look_at_folders();
        if !effects.is_empty() {
            self.kept.checked = Some(now);
        }
        effects
    }

    /// A worker looked at the recent folders and favorites: which are folders still (`true`) and
    /// which are gone.
    ///
    /// Returns the look that was asked for meanwhile, if one was; the next look starts only now.
    pub fn folders_checked(&mut self, found: &[(VPath, bool)]) -> Vec<Effect> {
        let again = self.kept.look.answered();
        for (folder, there) in found {
            if *there {
                self.kept.missing.remove(folder);
            } else {
                self.kept.missing.insert(folder.clone());
            }
        }
        let asked = self.folders_to_check();
        self.kept.missing.retain(|gone| asked.contains(gone));
        if again {
            return self.look_at_folders();
        }
        Vec::new()
    }

    /// Whether a worker found the recent folder or favorite `folder` gone: its row dims.
    pub fn is_missing(&self, folder: &VPath) -> bool {
        self.kept.missing.contains(folder)
    }

    /// Notes the folders the panes show now as visited, the active pane's last: a folder that
    /// is among the recent ones moves to the front. A pane that stands in at home for a folder
    /// not found yet has visited nothing. The controller calls this after every change.
    pub fn note_visits(&mut self) {
        let other = 1 - self.active;
        for side in [other, self.active] {
            let tab = self.tab(side);
            if tab.stands_in_for().is_some() {
                continue;
            }
            let location = tab.location().clone();
            if self.kept.visited[side].as_ref() == Some(&location) {
                continue;
            }
            self.kept.visited[side] = Some(location.clone());
            self.visit(&location);
        }
    }

    fn visit(&mut self, folder: &VPath) {
        self.kept.recent.retain(|recent| recent != folder);
        self.kept.recent.insert(0, folder.clone());
        self.kept.recent.truncate(RECENT_FOLDERS);
        // The folder is there: a worker found it gone once, perhaps, and it came back.
        self.kept.missing.remove(folder);
        if let Some(RecentAt::Item(at)) = self.kept.cursor {
            self.kept.cursor = Some(RecentAt::Item(at.min(self.kept.recent.len() - 1)));
        }
    }

    /// A click on the recent folder `index`: the active pane shows it, unless a worker found it
    /// gone, which a notice says.
    pub fn open_recent(&mut self, index: usize) -> Vec<Effect> {
        let Some(folder) = self.kept.recent.get(index).cloned() else {
            return Vec::new();
        };
        if self.kept.missing.contains(&folder) {
            return vec![Effect::Notice(words::start_folder_gone(&folder))];
        }
        self.go_to(folder)
    }

    /// The group of recent folders is folded to its head.
    pub fn recent_folded(&self) -> bool {
        self.kept.folded
    }

    pub fn set_recent_folded(&mut self, folded: bool) {
        self.kept.folded = folded;
        if folded && matches!(self.kept.cursor, Some(RecentAt::Item(_))) {
            self.kept.cursor = Some(RecentAt::Head);
        }
    }

    /// Folds the group of recent folders to its head, or opens it again.
    pub fn toggle_recent_fold(&mut self) -> Vec<Effect> {
        self.set_recent_folded(!self.kept.folded);
        Vec::new()
    }

    /// Where the keyboard cursor is in the group of recent folders, while it is.
    pub fn recent_cursor(&self) -> Option<RecentAt> {
        self.kept.cursor
    }

    /// Where the group of recent folders shows among the rows of the folder tree (see
    /// [`recent_slot`]); `None` while there is no recent folder, and no group.
    pub fn recent_slot(&self) -> Option<usize> {
        (!self.kept.recent.is_empty()).then(|| recent_slot(&self.tree.rows(), &self.places))
    }
}

/// `list` with `folders` put in from the gap `gap` on (`None`: at the end), in order, and the
/// place where the last one landed. A folder that is in the list already moves to its place
/// there; the others get the name of their folder.
fn put(mut list: Vec<Favorite>, folders: Vec<VPath>, gap: Option<usize>) -> (Vec<Favorite>, usize) {
    let mut at = gap.map(|gap| gap.min(list.len()));
    let mut landed = list.len();
    for folder in folders {
        let there = list
            .iter()
            .position(|favorite| favorite.folder().as_ref() == Some(&folder));
        landed = match (there, at) {
            (Some(from), Some(gap)) => {
                let to = if gap > from { gap - 1 } else { gap };
                let item = list.remove(from);
                list.insert(to, item);
                to
            }
            (Some(from), None) => {
                let item = list.remove(from);
                list.push(item);
                list.len() - 1
            }
            (None, Some(gap)) => {
                list.insert(gap, Favorite::new(folder_label(&folder), &folder));
                gap
            }
            (None, None) => {
                list.push(Favorite::new(folder_label(&folder), &folder));
                list.len() - 1
            }
        };
        at = at.map(|_| landed + 1);
    }
    (list, landed)
}
