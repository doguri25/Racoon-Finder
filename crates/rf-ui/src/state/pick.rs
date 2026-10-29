//! The destination picker (PICK) over the main window: opening it (F5, F6, the toolbar, the
//! row menu), what is chosen in it, its own folder tree, the worker's look at the target,
//! its 새 폴더, and the job its confirmation submits. A child of [`state`](super), so these
//! methods see the fields of [`AppState`].
//!
//! The picker's state is a [`Picker`] in the queue of windows ([`Modal::Pick`]); its folder
//! tree is a [`Tree`] of its own, [`AppState`]'s `pick_tree`, made anew each time a picker
//! opens and dropped when it closes, so that the listings it waits for stop.

use rf_core::{Entry, EntryKind, ErrorKind, VPath};
use rf_jobs::{ConflictPolicy, JobReport, JobSpec, Outcome, TransferMode};
use rf_vfs::VisibilityFilter;

use super::jobs::submit;
use super::{AppState, Effect, RIGHT};
use crate::dialogs::{Modal, NameDialog};
use crate::loader::LoadRequest;
use crate::pane::{Pane, Tab};
use crate::pick::{
    Looked, PickRequest, Picker, RECENT_TARGETS, TargetFacts, quick_targets, validate,
};
use crate::places::Place;
use crate::tree::{NodeId, Tree};
use crate::words;

impl AppState {
    /// The folders the picker offers as recent targets, the last one chosen first. Task 8
    /// keeps them between runs, with [`AppState::set_recent_targets`].
    pub fn recent_targets(&self) -> &[VPath] {
        &self.recent
    }

    /// Takes the recent targets of an earlier run, the last one chosen first: each once, the
    /// first [`RECENT_TARGETS`] of them.
    pub fn set_recent_targets(&mut self, targets: Vec<VPath>) {
        self.recent.clear();
        for target in targets {
            if self.recent.len() < RECENT_TARGETS && !self.recent.contains(&target) {
                self.recent.push(target);
            }
        }
    }

    /// The picker on screen: the window that shows, or the one under it while its 새 폴더
    /// asks for a name (or another window has come over it, like the question before the app
    /// ends).
    pub fn shown_picker(&self) -> Option<&Picker> {
        match (&self.modal, self.later.front()) {
            (Some(Modal::Pick(picker)), _) | (Some(_), Some(Modal::Pick(picker))) => Some(picker),
            _ => None,
        }
    }

    /// The picker's folder tree.
    pub fn pick_tree(&self) -> &Tree {
        &self.pick_tree
    }

    /// The folders the picker's filter finds ([`Tree::find`]), which its tree shows flat;
    /// `None` without a filter, when the tree shows as a tree.
    pub fn pick_found(&self) -> Option<Vec<NodeId>> {
        let filter = &self.shown_picker()?.filter;
        (!filter.trim().is_empty()).then(|| self.pick_tree.find(filter))
    }

    /// F5, F6, the toolbar's 복사… and 이동…, the row menu: the picker for the items of pane
    /// `side` (the selected rows, or the cursor's row; nothing without either), with the
    /// folder of the other pane chosen when it can take them (PICK-05). A worker looks at the
    /// target, and the picker's tree opens on the way to it.
    pub(super) fn open_picker(&mut self, side: usize, mode: TransferMode) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        let Some(request) = self.pick_request(side, mode) else {
            return Vec::new();
        };
        let opposite = self.tab(RIGHT - side).location().clone();
        let tabs: Vec<VPath> = self
            .panes
            .iter()
            .flat_map(Pane::tabs)
            .map(|tab| tab.location().clone())
            .collect();
        let groups = quick_targets(&request, &opposite, &tabs, &self.recent, &self.places);
        let picker = Picker::new(request, side, groups);
        let target = picker.target.clone();
        self.pick_tree = Tree::new(VisibilityFilter {
            show_hidden: self.show_hidden,
            ..VisibilityFilter::default()
        });
        self.open(Modal::Pick(Box::new(picker)));
        let roots = tree_roots(&self.places);
        let mut effects = self.with_pick_tree(|tree| tree.set_roots(roots));
        if let Some(target) = target {
            effects.extend(self.choose(target, None));
        }
        effects
    }

    /// A click on quick target `index`, counting every head's in turn: it becomes the
    /// target, unless it cannot be one.
    pub fn pick_clicked(&mut self, index: usize) -> Vec<Effect> {
        let Some(picker) = self.active_picker() else {
            return Vec::new();
        };
        let path = match picker.targets().nth(index) {
            Some(item) if item.validity.can_choose() => item.path.clone(),
            _ => return Vec::new(),
        };
        self.choose(path, None)
    }

    /// ↑ (`-1`) or ↓ (`1`) in the picker: the quick target before or after the target,
    /// passing over those that cannot be chosen.
    pub fn pick_step(&mut self, delta: isize) -> Vec<Effect> {
        let Some(picker) = self.active_picker() else {
            return Vec::new();
        };
        match picker.step(delta) {
            Some(path) if picker.target.as_ref() != Some(&path) => self.choose(path, None),
            _ => Vec::new(),
        }
    }

    /// Enter in the picker. After the filter changed, the first folder it finds that can take
    /// the items becomes the target, and the next Enter confirms; such a filter that finds
    /// none does nothing. Otherwise (no filter, or a target chosen since it changed) the
    /// picker is confirmed, unless its policy overwrites: only the button confirms what
    /// destroys files.
    pub fn pick_enter(&mut self) -> Vec<Effect> {
        let Some(picker) = self.active_picker() else {
            return Vec::new();
        };
        if picker.filter_fresh
            && let Some(found) = self.pick_found()
        {
            let first = found.into_iter().find_map(|id| {
                let path = &self.pick_tree.node(id)?.path;
                validate(&picker.request, path, None)
                    .can_choose()
                    .then(|| (id, path.clone()))
            });
            let Some((id, path)) = first else {
                return Vec::new();
            };
            if picker.target.as_ref() != Some(&path) {
                return self.choose(path, Some(id));
            }
            if let Some(picker) = self.active_picker_mut() {
                picker.filter_fresh = false;
            }
        }
        self.confirm_by_enter()
    }

    /// The picker's confirm button: its transfer is submitted with the policy chosen, the
    /// target goes first among the recent ones, and the picker closes (PICK-07). Nothing
    /// while the target cannot take the items.
    pub fn pick_confirmed(&mut self) -> Vec<Effect> {
        let Some(picker) = self.active_picker() else {
            return Vec::new();
        };
        if !picker.can_confirm() {
            return Vec::new();
        }
        let (Some(target), Some(spec)) = (picker.target.clone(), picker.spec()) else {
            return Vec::new();
        };
        let origin = picker.origin;
        self.close();
        self.remember_target(target);
        match spec {
            Ok(spec) => vec![submit(JobSpec::Transfer(spec), Some(origin))],
            Err(error) => vec![Effect::Notice(words::cannot_transfer(error.kind()))],
        }
    }

    /// The picker's 이동/복사 toggle.
    pub fn pick_mode(&mut self, mode: TransferMode) {
        if let Some(picker) = self.active_picker_mut() {
            picker.set_mode(mode);
        }
    }

    /// The picker's "이름이 겹치면" buttons.
    pub fn pick_policy(&mut self, policy: ConflictPolicy) {
        if let Some(picker) = self.active_picker_mut() {
            picker.policy = policy;
        }
    }

    /// The filter of the picker's tree changed to `text`. Once it is empty again, the tree
    /// shows as a tree, scrolled to the target's row.
    pub fn pick_filter(&mut self, text: &str) -> Vec<Effect> {
        let Some(picker) = self.active_picker_mut() else {
            return Vec::new();
        };
        picker.set_filter(text);
        if !text.trim().is_empty() {
            return Vec::new();
        }
        self.pick_tree
            .selected_row()
            .map(Effect::ScrollPickTree)
            .into_iter()
            .collect()
    }

    /// The path field now holds `text`, typed and not entered yet: unless it is the target's
    /// path, the picker confirms nothing until it is entered and found to be a folder, or
    /// another target is chosen (the field then shows that one).
    pub fn pick_path_edited(&mut self, text: &str) {
        if let Some(picker) = self.active_picker_mut() {
            picker.edit_path(text);
        }
    }

    /// Enter in the picker's path field with `text`: a worker looks at the path, which becomes
    /// the target if it is a folder, and the field says why not otherwise. Enter on the path of
    /// the target as it is does what Enter does elsewhere in the picker.
    pub fn pick_path_entered(&mut self, text: &str) -> Vec<Effect> {
        let Some(picker) = self.active_picker_mut() else {
            return Vec::new();
        };
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let Ok(path) = VPath::parse(text) else {
            picker.path_error = words::pick_path_error(ErrorKind::InvalidPath);
            return Vec::new();
        };
        if picker.target.as_ref() == Some(&path) && picker.typed.is_none() {
            // The field says the target, perhaps spelled another way: it shows it as it is.
            picker.path_error.clear();
            picker.path_edited = false;
            return self.confirm_by_enter();
        }
        let generation = picker.type_path(path.clone());
        let from_dir = picker.request.from_dir.clone();
        vec![Effect::InspectTarget {
            generation,
            path,
            from_dir,
        }]
    }

    /// A click on row `row` of the picker's tree (a row of what its filter finds, while it has
    /// one): its folder becomes the target. A folder that cannot take the items shows dimmed,
    /// with why, and takes no click, as among the quick targets; its chevron still opens it.
    pub fn pick_tree_clicked(&mut self, row: usize) -> Vec<Effect> {
        let Some(picker) = self.active_picker() else {
            return Vec::new();
        };
        let id = match self.pick_found() {
            Some(found) => found.get(row).copied(),
            None => self.pick_tree.node_at(row),
        };
        let Some(path) = id
            .and_then(|id| self.pick_tree.node(id))
            .map(|node| node.path.clone())
            .filter(|path| validate(&picker.request, path, None).can_choose())
        else {
            return Vec::new();
        };
        self.choose(path, id)
    }

    /// A click on the chevron of row `row` of the picker's tree: it opens or closes, listed
    /// the first time. The rows a filter finds have no chevron.
    pub fn pick_tree_toggled(&mut self, row: usize) -> Vec<Effect> {
        if self.active_picker().is_none() || self.pick_found().is_some() {
            return Vec::new();
        }
        match self.pick_tree.node_at(row) {
            Some(id) => self.with_pick_tree(|tree| tree.toggle(id).into_iter().collect()),
            None => Vec::new(),
        }
    }

    /// The picker's 새 폴더: the name window for a new folder in the target shows over the
    /// picker, which takes the folder as its target once the job has made it (PICK-03).
    /// Nothing while the path field says another folder than the target.
    pub fn pick_new_folder(&mut self) -> Vec<Effect> {
        let Some(parent) = self
            .active_picker()
            .filter(|picker| !picker.path_edited && picker.typed.is_none())
            .and_then(|picker| picker.target.clone())
        else {
            return Vec::new();
        };
        if let Some(picker) = self.modal.take() {
            self.later.push_front(picker);
        }
        self.modal = Some(Modal::Name(NameDialog::pick_folder(parent)));
        Vec::new()
    }

    /// What a worker found at `path` for the look `generation` of the picker
    /// ([`Effect::InspectTarget`]). A typed path that is a folder becomes the target, and the
    /// tree opens on the way to it; a late result is dropped.
    pub fn target_inspected(
        &mut self,
        generation: u64,
        path: &VPath,
        facts: TargetFacts,
    ) -> Vec<Effect> {
        let Some(picker) = self.picker_mut() else {
            return Vec::new();
        };
        match picker.looked(generation, path, facts) {
            Looked::Chosen => self.with_pick_tree(|tree| tree.reveal(path)),
            Looked::Stale | Looked::Facts | Looked::Refused => Vec::new(),
        }
    }

    /// A create ended: the folder the picker's 새 폴더 asked for becomes its target once it is
    /// made. A create that failed only says why ([`AppState::job_event`]).
    pub(super) fn pick_made(&mut self, report: &JobReport) -> Vec<Effect> {
        let JobSpec::Create { parent, name, .. } = &report.spec else {
            return Vec::new();
        };
        let Some(picker) = self.picker_mut() else {
            return Vec::new();
        };
        let asked = picker
            .making
            .as_ref()
            .is_some_and(|(folder, made)| folder == parent && made == name);
        if !asked {
            return Vec::new();
        }
        picker.making = None;
        match report.outputs.first() {
            Some(made) if report.outcome == Outcome::Completed => self.choose(made.clone(), None),
            _ => Vec::new(),
        }
    }

    /// The picker's tree is dropped once no picker is left in the queue of windows, and with
    /// it the listings it waits for.
    pub(super) fn drop_closed_picker(&mut self) {
        let open = self
            .modal
            .iter()
            .chain(&self.later)
            .any(|modal| matches!(modal, Modal::Pick(_)));
        if !open {
            self.pick_tree = Tree::new(VisibilityFilter::default());
        }
    }

    /// Changes the picker's tree with `f` and turns what it asks for into effects: the
    /// listings to start, and a scroll to the target's row when the tree has just found it
    /// (not while a filter shows other rows).
    pub(super) fn with_pick_tree(
        &mut self,
        f: impl FnOnce(&mut Tree) -> Vec<LoadRequest>,
    ) -> Vec<Effect> {
        let before = self.pick_tree.selected();
        let requests = f(&mut self.pick_tree);
        self.pick_tree_effects(before, requests)
    }

    /// The effects of `requests`, the listings the picker's tree asked for in a change whose
    /// selection was `before` it: see [`AppState::with_pick_tree`].
    pub(super) fn pick_tree_effects(
        &self,
        before: Option<NodeId>,
        requests: Vec<LoadRequest>,
    ) -> Vec<Effect> {
        let mut effects: Vec<Effect> = requests.into_iter().map(Effect::LoadChildren).collect();
        if self.pick_tree.selected() != before
            && self.pick_found().is_none()
            && let Some(row) = self.pick_tree.selected_row()
        {
            effects.push(Effect::ScrollPickTree(row));
        }
        effects
    }

    /// The request to copy or move (`mode`) the items of pane `side`: the selected rows, or
    /// the cursor's row; `None` without either.
    pub(super) fn pick_request(&self, side: usize, mode: TransferMode) -> Option<PickRequest> {
        let items = self.target_entries(side);
        if items.is_empty() {
            return None;
        }
        let mut request = PickRequest {
            mode,
            sources: Vec::with_capacity(items.len()),
            file_bytes: 0,
            folder_count: 0,
            other_count: 0,
            from_dir: self.tab(side).location().clone(),
        };
        for (path, entry) in items {
            match entry.kind {
                EntryKind::Dir => request.folder_count += 1,
                EntryKind::File => {
                    request.file_bytes = request.file_bytes.saturating_add(entry.size);
                }
                // A link moves as a link, and something else as it is: no size to add.
                EntryKind::Symlink | EntryKind::Other => request.other_count += 1,
            }
            request.sources.push(path);
        }
        Some(request)
    }

    /// The items a command acts on in pane `side`, with their rows: the selected rows, or
    /// the cursor's row when none is selected and the user put the cursor there (not the
    /// first row of a folder just opened, nor a row the user unselected since:
    /// [`Selection::user_cursor`]).
    ///
    /// [`Selection::user_cursor`]: crate::selection::Selection::user_cursor
    pub(super) fn target_entries(&self, side: usize) -> Vec<(VPath, &Entry)> {
        let tab: &Tab = self.tab(side);
        let mut rows = tab.selection().indices(tab.visible());
        if rows.is_empty() {
            rows.extend(
                tab.selection()
                    .user_cursor()
                    .filter(|&row| row < tab.visible().len()),
            );
        }
        rows.into_iter()
            .filter_map(|row| {
                let entry = &tab.visible()[row];
                Some((tab.location().join(&entry.name).ok()?, entry))
            })
            .collect()
    }

    /// Makes `path` the picker's target: a worker looks at it, and the picker's tree shows it
    /// (selects node `node` when the click was on it, else opens the folders on the way).
    fn choose(&mut self, path: VPath, node: Option<NodeId>) -> Vec<Effect> {
        let Some(picker) = self.picker_mut() else {
            return Vec::new();
        };
        let generation = picker.choose(path.clone());
        let from_dir = picker.request.from_dir.clone();
        let mut effects = match node {
            Some(id) => {
                self.pick_tree.select(id);
                Vec::new()
            }
            None => self.with_pick_tree(|tree| tree.reveal(&path)),
        };
        effects.push(Effect::InspectTarget {
            generation,
            path,
            from_dir,
        });
        effects
    }

    /// Confirms the picker from the keyboard: not when its policy overwrites.
    fn confirm_by_enter(&mut self) -> Vec<Effect> {
        match self.active_picker() {
            Some(picker) if picker.policy != ConflictPolicy::Overwrite => self.pick_confirmed(),
            _ => Vec::new(),
        }
    }

    /// Puts `target` first among the recent targets.
    fn remember_target(&mut self, target: VPath) {
        self.recent.retain(|recent| *recent != target);
        self.recent.insert(0, target);
        self.recent.truncate(RECENT_TARGETS);
    }

    /// The picker, while it is the window that shows.
    fn active_picker(&self) -> Option<&Picker> {
        match &self.modal {
            Some(Modal::Pick(picker)) => Some(picker),
            _ => None,
        }
    }

    fn active_picker_mut(&mut self) -> Option<&mut Picker> {
        match &mut self.modal {
            Some(Modal::Pick(picker)) => Some(picker),
            _ => None,
        }
    }

    /// The picker in the queue of windows, shown or waiting.
    fn picker_mut(&mut self) -> Option<&mut Picker> {
        self.modal
            .iter_mut()
            .chain(self.later.iter_mut())
            .find_map(|modal| match modal {
                Modal::Pick(picker) => Some(&mut **picker),
                _ => None,
            })
    }
}

/// The roots of a folder tree for `places`: their names and folders.
pub(super) fn tree_roots(places: &[Place]) -> Vec<(String, VPath)> {
    places
        .iter()
        .map(|place| (place.label.clone(), place.path.clone()))
        .collect()
}
