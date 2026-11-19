//! 휴지통 and 내 PC in the panes (NAV-16). A child of [`state`](super), so these methods see the
//! fields of [`AppState`].
//!
//! Both are places of the shell (`shell://trash`, `shell://computer`): a tab shows them as it
//! shows a folder, with a path row and a path field that name them (`휴지통`, `내 PC`), its
//! history and its back and forward, but they are lists, not folders. What the commands may do
//! there is what the shell provider's capability bits say ([`place_capabilities`]): in the trash
//! putting items back (`RESTORE`) and deleting them for good (`DELETE_FOREVER`), and nothing that
//! writes into a folder (no `WRITE`); in the computer nothing at all. A command the bits leave
//! out says so in a notice ([`AppState::shell_refusal`]), never silently.
//!
//! - **The trash** lists its items by id, each with the record its listing read
//!   ([`Tab::trash_item`](crate::pane::Tab::trash_item)), which a restore or a delete for good
//!   carries to the job: the job acts on the item only while the trash still holds that very item
//!   (rf-platform's stamp), whatever happened to the trash since the list was read. Its menu
//!   offers 복원 (to the original place; off when the trash does not keep one for an item),
//!   위치를 골라 꺼내기… (the picker) and 영구 삭제…, and its empty part 휴지통 비우기…. The two
//!   that delete for good ask first, saying how many items, their size when known and that it
//!   cannot be undone; only their button confirms ([`Confirm::danger`]). A restore is an undoable
//!   job: Ctrl+Z sends the items back to the trash.
//! - **The trash cannot be watched**: every tab that shows it lists it again after a job of the
//!   app's that changes it ends ([`changes_trash`]), when the window comes to the front
//!   ([`AppState::window_focused`]), and on F5. A row may still be stale (another app emptied
//!   the trash meanwhile): the job then fails with `TrashItemChanged` and says so, and its end
//!   lists the trash again.
//! - **The computer** lists the drives of the sidebar (no look of its own at the disks), as tiles
//!   with the bar of how full each is; opening one goes to its top folder. F5 there finds the
//!   drives again ([`Effect::FindPlaces`]).

use std::ffi::OsString;

use rf_core::{EntryKind, VPath};
use rf_jobs::{FailureReason, JobReport, JobSpec, Outcome, UndoRecord};
use rf_vfs::{Capabilities, ShellPlace, TrashItem, drive_name};

use super::jobs::submit;
use super::{AppState, Effect, ToastAction};
use crate::dialogs::{Confirm, Modal};
use crate::format::{folder_label, shell_place_at};
use crate::keys::Command;
use crate::loader::{LoadEvent, LoadRequest};
use crate::pick::{PickPurpose, PickRequest, Picker, quick_targets};
use crate::places::{Place, PlaceKind};
use crate::words::{self, Blocked};

/// The names of the commands of the trash's menus ([`AppState::trash_command`]): 복원,
/// 위치를 골라 꺼내기…, 영구 삭제… and 휴지통 비우기….
pub const TRASH_COMMANDS: [&str; 4] = [
    "restore",
    "restore-elsewhere",
    "delete-from-trash",
    "empty-trash",
];

/// What the place of the shell `folder` is lets commands do there, as its provider's capability
/// bits say (`ShellPlace::capabilities`); `None` for a folder of a drive, whose provider decides
/// item by item (the job finds out).
pub fn place_capabilities(folder: &VPath) -> Option<Capabilities> {
    ShellPlace::of(folder).map(ShellPlace::capabilities)
}

/// The rows of 내 PC for the drives among `places` (the sidebar's, which a worker found), in
/// their order: each named by its id there (`shell://computer/<id>`, as the shell provider names
/// it), shown by the name the sidebar gives it, with its top folder as its origin and its size.
pub fn computer_entries(places: &[Place]) -> Vec<rf_core::Entry> {
    places
        .iter()
        .filter(|place| place.kind == PlaceKind::Drive)
        .map(|place| {
            let mut entry = rf_core::Entry::new(drive_name(&place.path), EntryKind::Dir);
            entry.label = Some(OsString::from(&place.label));
            entry.origin = place.path.as_local().map(std::path::Path::to_path_buf);
            entry.size = place.space.map_or(0, |space| space.total);
            entry
        })
        .collect()
}

/// Whether a job of `spec` changes what the trash holds: a delete to the trash, a restore, a
/// delete from the trash and its emptying, and an undo or a redo, which may send items to the
/// trash or bring them back. The tabs that show the trash list it again when it ends.
pub fn changes_trash(spec: &JobSpec) -> bool {
    match spec {
        JobSpec::Delete(delete) => delete.method == rf_jobs::DeleteMethod::Trash,
        JobSpec::RestoreFromTrash { .. }
        | JobSpec::DeleteFromTrash { .. }
        | JobSpec::EmptyTrash
        | JobSpec::Undo(_) => true,
        _ => false,
    }
}

/// The restore of `items` of the trash into `folder`, each under the name it had (위치를 골라
/// 꺼내기…), asked for in pane `origin`.
pub(super) fn restore_into(items: Vec<TrashItem>, folder: &VPath, origin: usize) -> Vec<Effect> {
    let Some(local) = folder.as_local() else {
        return vec![Effect::Notice(words::shell_blocked(
            ShellPlace::Trash,
            Blocked::Transfer,
        ))];
    };
    let items = items
        .into_iter()
        .map(|item| {
            let to = local.join(&item.name);
            (item, to)
        })
        .collect();
    vec![submit(JobSpec::RestoreFromTrash { items }, Some(origin))]
}

impl AppState {
    /// The place of the shell that pane `side` shows, if it shows one.
    pub(super) fn shell_place_of(&self, side: usize) -> Option<ShellPlace> {
        shell_place_at(self.tab(side).location())
    }

    /// The listing of 내 PC for the tab whose request is `generation` (the controller's answer
    /// to its [`Effect::Load`]): the drives of the sidebar ([`computer_entries`]), with no look
    /// at the disks. Before the places are found it lists none, and lists them once they are
    /// ([`AppState::set_places`]).
    pub fn list_computer(&mut self, generation: u64) -> Vec<Effect> {
        let entries = computer_entries(&self.places);
        self.loaded(LoadEvent::Listed {
            generation,
            entries,
        })
    }

    /// Every tab that shows `place` lists it again, keeping its rows meanwhile; what waits for
    /// one of their listings waits for the new one ([`AppState::relisted`]).
    pub(super) fn relist_shell(&mut self, place: ShellPlace) -> Vec<Effect> {
        let moves: Vec<(u64, LoadRequest)> = self
            .panes
            .iter_mut()
            .flat_map(crate::pane::Pane::tabs_mut)
            .filter(|tab| shell_place_at(tab.location()) == Some(place))
            .map(|tab| (tab.generation(), tab.reload()))
            .collect();
        moves
            .into_iter()
            .map(|(old, request)| {
                self.relisted(old, request.generation);
                Effect::Load(request)
            })
            .collect()
    }

    /// The tab whose listing was `old` lists its folder again under `generation` (a relist of
    /// the trash, F5, a rescan, a reload after its watch ended): navigation sync's wait, and the
    /// question 휴지통 비우기… waits to ask, go on under the new listing, so that neither is lost.
    pub(super) fn relisted(&mut self, old: u64, generation: u64) {
        self.sync_relisted(old, generation);
        if self.pending_empty == Some(old) {
            self.pending_empty = Some(generation);
        }
    }

    /// A job of `spec` ended: when it changes the trash ([`changes_trash`]), which cannot be
    /// watched, the tabs that show the trash list it again, so that what it did shows.
    pub(super) fn trash_changed(&mut self, spec: &JobSpec) -> Vec<Effect> {
        if self.quitting || !changes_trash(spec) {
            return Vec::new();
        }
        self.relist_shell(ShellPlace::Trash)
    }

    /// The window came to the front: the tabs that show the trash, which cannot be watched, list
    /// it again, so that what another app put in it or took out shows.
    pub fn window_focused(&mut self) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        self.relist_shell(ShellPlace::Trash)
    }

    /// What `command` does in pane `side` when the pane shows a place of the shell, in the
    /// place of what it does in a folder; `None` for a folder, and for a command that works
    /// there as anywhere (the cursor, the selection, the tabs, the view, F5). The place's
    /// capability bits ([`place_capabilities`]) decide: without `WRITE` nothing is made,
    /// renamed, pasted or moved in or out; with `DELETE_FOREVER` (the trash) Delete deletes for
    /// good after its question, and without `TRASH` nothing goes to a trash from there. The items
    /// of a place of the shell are no files to read (its provider refuses `open_read`), so
    /// nothing copies, previews or shows the properties of them either. Each refusal says why in
    /// a notice.
    pub(super) fn shell_refusal(&mut self, side: usize, command: &Command) -> Option<Vec<Effect>> {
        let place = self.shell_place_of(side)?;
        let caps = place.capabilities();
        let writes = caps.contains(Capabilities::WRITE);
        let blocked = match command {
            Command::Open => return Some(self.open_cursor_in_shell(side, place)),
            Command::Refresh if place == ShellPlace::Computer => {
                let mut effects = self.refresh(side);
                effects.push(Effect::FindPlaces);
                return Some(effects);
            }
            // In the trash, Delete deletes for good, after its question.
            Command::Trash | Command::DeletePermanently
                if caps.contains(Capabilities::DELETE_FOREVER) =>
            {
                return Some(self.delete_from_trash(side));
            }
            Command::Trash if !caps.contains(Capabilities::TRASH) => Blocked::Delete,
            Command::DeletePermanently if !writes => Blocked::Delete,
            Command::NewFolder if !writes => Blocked::NewFolder,
            Command::Rename if !writes => Blocked::Rename,
            Command::Paste | Command::PasteMove if !writes => Blocked::Paste,
            // A move takes the items out, which is a write too; a copy reads them.
            Command::CopyTo | Command::MoveTo => Blocked::Transfer,
            Command::Copy => Blocked::Copy,
            Command::Cut => Blocked::Cut,
            Command::CopyPath => Blocked::CopyPath,
            Command::Properties | Command::SystemProperties => Blocked::Properties,
            Command::OpenWith | Command::RunAsAdmin => Blocked::OpenWith,
            Command::OpenTerminal => Blocked::Terminal,
            Command::QuickLook => Blocked::Preview,
            // The system's menu of items knows no list of the shell: the app's menu shows.
            Command::SystemMenu | Command::ContextMenu => return Some(self.app_menu(side)),
            _ => return None,
        };
        Some(vec![Effect::Notice(words::shell_blocked(place, blocked))])
    }

    /// The notice of `what` in pane `side` when it shows a place of the shell, for a command
    /// that does not go through the keys (a menu's, a toolbar's); `None` for a folder. While a
    /// window shows over the main one, or the app ends, the command does nothing there, and says
    /// nothing under the window (as the keys do then).
    pub(super) fn blocked_in_shell(&self, side: usize, what: Blocked) -> Option<Vec<Effect>> {
        let place = self.shell_place_of(side)?;
        if self.modal.is_some() || self.quitting {
            return Some(Vec::new());
        }
        Some(vec![Effect::Notice(words::shell_blocked(place, what))])
    }

    /// Enter, or the menu's 열기, in pane `side` that shows `place`: a drive of the computer
    /// opens at its top folder; an item of the trash does not open (it is not a file anywhere
    /// yet), and a notice says to restore it first.
    fn open_cursor_in_shell(&mut self, side: usize, place: ShellPlace) -> Vec<Effect> {
        let row = self.tab(side).selection().cursor();
        match row {
            Some(row) => self.open_row_in_shell(side, row, place),
            None => Vec::new(),
        }
    }

    /// A double click or Enter on row `row` of pane `side` that shows `place`
    /// ([`AppState::open_cursor_in_shell`]).
    pub(super) fn open_row_in_shell(
        &mut self,
        side: usize,
        row: usize,
        place: ShellPlace,
    ) -> Vec<Effect> {
        match place {
            ShellPlace::Trash => vec![Effect::Notice(words::trash_item_not_opened().to_owned())],
            ShellPlace::Computer => {
                let root = self
                    .tab(side)
                    .visible()
                    .get(row)
                    .and_then(|entry| entry.origin.as_deref())
                    .and_then(|origin| VPath::local(origin).ok());
                match root {
                    Some(root) => super::load(self.navigate_shown(side, root)),
                    None => vec![Effect::Notice(words::drive_gone().to_owned())],
                }
            }
        }
    }

    /// The records of the trash's items that a command acts on in pane `side` (the selected
    /// rows, or the cursor's row the user put there), as the list's listing read them; none when
    /// the pane does not show the trash.
    pub(super) fn trash_targets(&self, side: usize) -> Vec<TrashItem> {
        if self.shell_place_of(side) != Some(ShellPlace::Trash) {
            return Vec::new();
        }
        let tab = self.tab(side);
        self.target_entries(side)
            .into_iter()
            .filter_map(|(_, entry)| tab.trash_item(&entry.name).cloned())
            .collect()
    }

    /// Whether every item 복원 would act on in pane `side` has an original place the trash
    /// keeps: the menu offers 복원 only then, and 위치를 골라 꺼내기… always.
    pub fn can_restore(&self, side: usize) -> bool {
        let items = self.trash_targets(side);
        !items.is_empty() && items.iter().all(|item| item.original.is_some())
    }

    /// 복원 of the trash's menu: the items the active pane's command acts on go back where they
    /// were, a job that asks about a name that is taken there (skip, a numbered name, or
    /// stopping: it never replaces) and that Ctrl+Z undoes. Items whose original place the trash
    /// does not keep are not restored: a notice says to take them out with
    /// 위치를 골라 꺼내기… (the menu offers only that for them).
    pub fn restore_items(&mut self) -> Vec<Effect> {
        if self.quitting {
            return Vec::new();
        }
        let side = self.active;
        let items = self.trash_targets(side);
        if items.is_empty() {
            return Vec::new();
        }
        let Some(items) = items
            .into_iter()
            .map(|item| {
                let to = item.original.clone()?;
                Some((item, to))
            })
            .collect::<Option<Vec<_>>>()
        else {
            return vec![Effect::Notice(words::restore_place_unknown().to_owned())];
        };
        vec![submit(JobSpec::RestoreFromTrash { items }, Some(side))]
    }

    /// 위치를 골라 꺼내기… of the trash's menu: the picker finds the folder the items the active
    /// pane's command acts on go into, each under the name it had.
    pub fn restore_elsewhere(&mut self) -> Vec<Effect> {
        let side = self.active;
        let items = self.trash_targets(side);
        self.pick_restore(side, items)
    }

    /// The picker for taking `items` of the trash out, asked for in pane `side`: its request is
    /// made of the items themselves (their rows, wherever the pane is now), as a move out of the
    /// trash.
    fn pick_restore(&mut self, side: usize, items: Vec<TrashItem>) -> Vec<Effect> {
        if self.quitting || items.is_empty() {
            return Vec::new();
        }
        let trash = ShellPlace::Trash.path();
        let request = PickRequest {
            mode: rf_jobs::TransferMode::Move,
            sources: items
                .iter()
                .filter_map(|item| trash.join(item.id.as_str()).ok())
                .collect(),
            file_bytes: items
                .iter()
                .filter(|item| !item.is_dir)
                .filter_map(|item| item.size)
                .fold(0, u64::saturating_add),
            folder_count: items.iter().filter(|item| item.is_dir).count(),
            other_count: 0,
            from_dir: trash,
        };
        let opposite = self.tab(super::RIGHT - side).location().clone();
        let tabs = self.open_tab_folders();
        let groups = quick_targets(&request, &opposite, &tabs, &self.recent, &self.places);
        let picker = Picker::for_purpose(PickPurpose::Restore { items }, request, side, groups);
        self.show_picker(picker)
    }

    /// 영구 삭제… in the trash (and Delete there): the question before the items the command acts
    /// on in pane `side` are deleted for good ([`Confirm::delete_from_trash`]). Nothing is
    /// deleted before its button is pressed.
    pub(super) fn delete_from_trash(&mut self, side: usize) -> Vec<Effect> {
        let items = self.trash_targets(side);
        if !items.is_empty() {
            self.open(Modal::Confirm(Confirm::delete_from_trash(items)));
        }
        Vec::new()
    }

    /// 영구 삭제… of the trash's menu, for the active pane.
    pub fn delete_from_trash_menu(&mut self) -> Vec<Effect> {
        let side = self.active;
        self.delete_from_trash(side)
    }

    /// 휴지통 비우기… of the trash's menu: the trash of the active pane is listed again first,
    /// so that the question says how many items it holds now; the question opens when that
    /// listing comes ([`AppState::empty_listed`]). Nothing is deleted before its button is
    /// pressed. Nothing outside the trash.
    pub fn empty_trash(&mut self) -> Vec<Effect> {
        let side = self.active;
        if self.quitting || self.shell_place_of(side) != Some(ShellPlace::Trash) {
            return Vec::new();
        }
        let request = self.tab_mut(side).reload();
        self.pending_empty = Some(request.generation);
        vec![Effect::Load(request)]
    }

    /// The listing `generation` of a tab came (`ended`: whole or failed): when 휴지통 비우기…
    /// waits for it, its question opens with the items as they are now, or a notice says that the
    /// trash is empty, or could not be read.
    pub(super) fn empty_listed(&mut self, generation: u64) -> Vec<Effect> {
        if self.pending_empty != Some(generation) {
            return Vec::new();
        }
        self.pending_empty = None;
        let Some(tab) = self
            .panes
            .iter()
            .flat_map(crate::pane::Pane::tabs)
            .find(|tab| tab.generation() == generation)
        else {
            return Vec::new();
        };
        if let crate::pane::LoadStatus::Failed(kind) = tab.status() {
            return vec![Effect::Notice(words::trash_unreadable(kind))];
        }
        // Every item, the hidden ones and those a filter hides too: they all go.
        let items: Vec<TrashItem> = tab
            .all()
            .iter()
            .filter_map(|entry| tab.trash_item(&entry.name).cloned())
            .collect();
        if items.is_empty() {
            return vec![Effect::Notice(words::trash_already_empty().to_owned())];
        }
        self.open(Modal::Confirm(Confirm::empty_trash(items)));
        Vec::new()
    }

    /// The command of the trash's menus `name` (one of [`TRASH_COMMANDS`]); `None` for another
    /// name.
    pub fn trash_command(&mut self, name: &str) -> Option<Vec<Effect>> {
        Some(match name {
            "restore" => self.restore_items(),
            "restore-elsewhere" => self.restore_elsewhere(),
            "delete-from-trash" => self.delete_from_trash_menu(),
            "empty-trash" => self.empty_trash(),
            _ => return None,
        })
    }

    /// The toast of a restore that ended (NAV-16), when it has no failure, or only items whose
    /// original folder is gone: where the items went, with the button that undoes it; for those
    /// whose folder is gone, the button that takes them out elsewhere (위치를 골라 꺼내기…).
    /// `None` for another job, or one with other failures (the usual notice tells of them).
    pub(super) fn restore_toast(&mut self, report: &JobReport) -> Option<Effect> {
        let JobSpec::RestoreFromTrash { items } = &report.spec else {
            return None;
        };
        let gone: Vec<TrashItem> = items
            .iter()
            .filter(|(item, _)| {
                report.failures.iter().any(|failure| {
                    failure.reason == Some(FailureReason::OriginalFolderGone)
                        && failure.source.as_ref().and_then(VPath::as_local)
                            == Some(item.location.as_path())
                })
            })
            .map(|(item, _)| item.clone())
            .collect();
        if gone.len() != report.failures.len() {
            return None;
        }
        if !gone.is_empty() {
            let text = words::restore_folders_gone(gone.len());
            return Some(self.toast(text, Some(ToastAction::RestoreElsewhere(gone))));
        }
        if report.outcome != Outcome::Completed {
            return None;
        }
        let restored = report.undo.as_ref().map_or(0, UndoRecord::item_count);
        if restored == 0 {
            return None;
        }
        // The one folder they went into, when there is one.
        let mut folders = report.outputs.iter().filter_map(VPath::parent);
        let first = folders.next();
        let one = first
            .as_ref()
            .filter(|first| folders.all(|folder| folder == **first))
            .map(folder_label);
        let text = words::restored(restored, one.as_deref());
        Some(self.toast(text, report.undo.clone().map(ToastAction::Undo)))
    }

    /// The button of the toast of a restore whose original folders were gone: the picker finds
    /// a place for `items`.
    pub(super) fn restore_elsewhere_items(&mut self, items: Vec<TrashItem>) -> Vec<Effect> {
        let side = self.active;
        self.pick_restore(side, items)
    }
}
