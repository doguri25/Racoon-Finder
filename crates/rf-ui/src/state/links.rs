//! Making links (OPS-08): the item menu's 링크 만들기 ▸ (a shortcut or alias, a symbolic link,
//! on Windows a junction for a folder) and the job a drop with the link keys submits. A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! A link is a job like any other (`JobSpec::MakeLink`, with its undo record), made by a worker:
//! the state only says what the menu offers ([`link_facts`]) and builds the job ([`link_job`]).
//! The menu makes the links in the folder the pane shows; with Shift held as the kind is chosen,
//! the picker finds the folder instead (PICK-09, [`PickPurpose::Link`]). A symbolic link on
//! Windows with no right to make one is shown but off, and says why; no job tries it.

use rf_core::{Entry, EntryKind, Error, Os, VPath};
use rf_jobs::JobSpec;
use rf_platform::{LinkKind, kinds_for, link_name};

use super::jobs::submit;
use super::{AppState, Effect, RIGHT};
use crate::pick::{PickPurpose, Picker, quick_targets};
use crate::words;

/// One entry of 링크 만들기 ▸.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LinkChoice {
    pub kind: LinkKind,
    /// Off where the link cannot be made now: the title says why.
    pub enabled: bool,
}

/// The kinds of link the item menu offers for the items it opened for, in the order it lists
/// them. At most three.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LinkFacts {
    choices: [Option<LinkChoice>; 3],
}

impl LinkFacts {
    /// The entries, in the order the menu lists them.
    pub fn choices(&self) -> impl Iterator<Item = LinkChoice> + '_ {
        self.choices.iter().flatten().copied()
    }

    /// The entry at `index` of the menu.
    pub fn choice(&self, index: usize) -> Option<LinkChoice> {
        self.choices().nth(index)
    }
}

/// What the item menu offers of 링크 만들기 ▸ for `items` on `os`, where a symbolic link
/// needs rights the process lacks (`symlink_needs_privilege`) or not. Nothing for no item, or for
/// an item that is not on the local disk. A junction only where every item is a folder itself (a
/// link is not known to lead to one, and the UI thread does not look).
pub fn link_facts(os: Os, items: &[(VPath, &Entry)], symlink_needs_privilege: bool) -> LinkFacts {
    let local = items.iter().all(|(path, _)| path.as_local().is_some());
    if items.is_empty() || !local {
        return LinkFacts::default();
    }
    let all_dirs = items.iter().all(|(_, entry)| entry.kind == EntryKind::Dir);
    let mut choices = [None; 3];
    for (slot, kind) in choices.iter_mut().zip(kinds_for(os, all_dirs)) {
        *slot = Some(LinkChoice {
            kind,
            enabled: kind != LinkKind::Symlink || !symlink_needs_privilege,
        });
    }
    LinkFacts { choices }
}

/// The job that makes a link of `kind` to each of `targets` in `folder`, each under the name
/// `os` gives it ([`link_name`]). `Unsupported` for a target with no name (a volume root).
pub fn link_job(
    os: Os,
    kind: LinkKind,
    targets: &[VPath],
    folder: &VPath,
) -> Result<JobSpec, Error> {
    let mut links = Vec::with_capacity(targets.len());
    for target in targets {
        let name = target
            .file_name()
            .ok_or_else(|| Error::Unsupported(format!("cannot link a volume root: {target}")))?;
        links.push((
            target.clone(),
            folder.join(link_name(name, kind, os))?,
            kind,
        ));
    }
    Ok(JobSpec::MakeLink { links })
}

impl AppState {
    /// What 링크 만들기 ▸ offers for the items the item menu opened for last.
    pub fn item_links(&self) -> LinkFacts {
        self.shell.links
    }

    /// Entry `index` of 링크 만들기 ▸ was chosen, for the items a command acts on in the active
    /// pane: links to them in the folder it shows, or with `pick` (Shift held) in a folder the
    /// picker finds. An entry that is off says why and makes nothing.
    pub fn make_links(&mut self, index: usize, pick: bool) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        let Some(choice) = self.shell.links.choice(index) else {
            return Vec::new();
        };
        if !choice.enabled {
            return vec![Effect::Notice(words::link_needs_privilege().to_owned())];
        }
        let side = self.active;
        let targets: Vec<VPath> = self
            .target_entries(side)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        if pick {
            return self.open_link_picker(side, choice.kind);
        }
        let folder = self.tab(side).location().clone();
        match link_job(Os::CURRENT, choice.kind, &targets, &folder) {
            Ok(spec) => vec![submit(spec, Some(side))],
            Err(error) => vec![Effect::Notice(words::cannot_transfer(error.kind()))],
        }
    }

    /// The picker that finds the folder for links of `kind` to the items of pane `side`
    /// (PICK-09), with the folder of the other pane chosen when it can take them. Nothing is
    /// copied, so no room is needed there.
    fn open_link_picker(&mut self, side: usize, kind: LinkKind) -> Vec<Effect> {
        let Some(mut request) = self.pick_request(side, rf_jobs::TransferMode::Copy) else {
            return Vec::new();
        };
        request.file_bytes = 0;
        let opposite = self.tab(RIGHT - side).location().clone();
        let tabs = self.open_tab_folders();
        let groups = quick_targets(&request, &opposite, &tabs, &self.recent, &self.places);
        self.show_picker(Picker::for_purpose(
            PickPurpose::Link { kind },
            request,
            side,
            groups,
        ))
    }
}
