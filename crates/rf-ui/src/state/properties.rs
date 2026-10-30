//! The properties window (OPS-06's own window) and the checksums of its files (OPS-10). A child
//! of [`state`](super), so these methods see the fields of [`AppState`].
//!
//! Alt+Enter (⌘I on macOS) and 속성 in the menu of a row open the window over the main window
//! ([`Modal::Properties`]) for the items a command acts on in the active pane (the selected
//! rows, or the row the user put the cursor on), or, with none, for the folder the pane shows
//! ([`crate::properties`]); 속성 in the menu of the part of a list without rows opens it for that
//! folder whatever is chosen ([`AppState::folder_properties`]). It only reads: nothing in it changes a file. While it shows, the
//! panes take no keys or clicks, so the items it shows stay the ones chosen.
//!
//! Two workers add what the listing does not tell, each request under a generation of its own:
//! the measure ([`Effect::Measure`]: the entry of the folder the pane shows, where a lone link
//! leads, and how much the folders hold) and the checksums of the files with the algorithm
//! chosen ([`Effect::Hash`]). A worker's news is taken only while its generation is the
//! window's ([`AppState::measure_news`], [`AppState::hash_news`]). The window's work stops:
//!
//! - when the window closes, whichever way (Esc, 닫기, the app's end): the controller calls
//!   [`AppState::follow_properties`] after every change, which then stops both workers;
//! - the count, on its 중지 ([`AppState::stop_count`]), which keeps what it found;
//! - the checksums, on their 중지 ([`AppState::stop_hashes`]), which keeps the checksums made
//!   (the same algorithm chosen again makes the rest), and when another algorithm is chosen:
//!   the new request replaces the old one on the worker, which stops it, and the old one's late
//!   news has the old generation.
//!
//! A new window has new generations: nothing of the items of a window before it comes into it.

use rf_core::{Attributes, EntryKind, ErrorKind};

use super::{AppState, ClipboardFailure, ClipboardWork, Effect};
use crate::dialogs::Modal;
use crate::hashing::{Algorithm, HashNews, HashRequest};
use crate::pane::next_generation;
use crate::properties::{
    CountStatus, HashStatus, Item, MeasureItem, MeasureNews, MeasureRequest, Properties, Tally,
};

/// What the window has besides itself.
#[derive(Debug, Default)]
pub(super) struct PropertiesWork {
    /// The window, while it shows or waits behind another one.
    shown: Option<Properties>,
    /// The token of the last copy of a checksum, with its row.
    copying: Option<(u64, usize)>,
    token: u64,
}

impl AppState {
    /// The properties window, while it shows (or waits behind the question whether to quit).
    pub fn properties(&self) -> Option<&Properties> {
        self.properties.shown.as_ref()
    }

    /// Alt+Enter (⌘I): the properties window for the items of pane `side`, or for its folder
    /// when it has none, and the request for what a worker is to find out of them.
    pub(super) fn open_properties(&mut self, side: usize) -> Vec<Effect> {
        self.properties_of(side, true)
    }

    /// 속성 in the menu of the part of a list without rows: the properties window for the
    /// folder the active pane shows, whatever is chosen in it, as File Explorer's.
    pub fn folder_properties(&mut self) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        self.properties_of(self.active, false)
    }

    /// The properties window for the items of pane `side` (`chosen`, when it has some), or else
    /// for its folder.
    fn properties_of(&mut self, side: usize, chosen: bool) -> Vec<Effect> {
        let folder = self.tab(side).location().clone();
        let chosen = if chosen {
            self.target_entries(side)
        } else {
            Vec::new()
        };
        let own_folder = chosen.is_empty();
        let items: Vec<Item> = if own_folder {
            vec![Item {
                path: folder.clone(),
                entry: None,
                unreadable: None,
                unlisted: None,
                link: None,
            }]
        } else {
            chosen
                .into_iter()
                .map(|(path, entry)| Item {
                    path,
                    entry: Some(entry.clone()),
                    unreadable: None,
                    unlisted: None,
                    link: None,
                })
                .collect()
        };
        let mut properties = Properties::new(next_generation(), folder, own_folder, items);
        // A file stored online only is not read: reading it would download it.
        for row in &mut properties.hashing.rows {
            let online_only = properties.items[row.item]
                .entry
                .as_ref()
                .is_some_and(|entry| entry.attributes.contains(Attributes::OFFLINE));
            if online_only {
                row.status = HashStatus::OnlineOnly;
            }
        }
        let single = properties.items.len() == 1;
        let counts = own_folder || properties.chosen.folders > 0;
        let links = single
            && properties.items[0]
                .entry
                .as_ref()
                .is_some_and(|entry| entry.kind == EntryKind::Symlink);
        let mut effects = Vec::new();
        if counts || links {
            let generation = next_generation();
            properties.count.generation = generation;
            properties.measuring = true;
            if counts {
                properties.count.status = CountStatus::Counting;
            }
            effects.push(Effect::Measure(MeasureRequest {
                generation,
                items: properties
                    .items
                    .iter()
                    .map(|item| MeasureItem {
                        path: item.path.clone(),
                        entry: item.entry.clone(),
                    })
                    .collect(),
                link_targets: single,
            }));
        }
        self.properties.shown = Some(properties);
        self.properties.copying = None;
        self.open(Modal::Properties);
        effects
    }

    /// Stops the work of a properties window that has gone (closed, or dropped as the app
    /// ends), and forgets it. The controller calls this after every change.
    pub fn follow_properties(&mut self) -> Vec<Effect> {
        let open = self
            .modal
            .iter()
            .chain(&self.later)
            .any(|modal| matches!(modal, Modal::Properties));
        if open {
            return Vec::new();
        }
        let Some(gone) = self.properties.shown.take() else {
            return Vec::new();
        };
        self.properties.copying = None;
        let mut effects = Vec::new();
        if gone.measuring {
            effects.push(Effect::StopMeasure);
        }
        if gone.hashing.under_way() {
            effects.push(Effect::StopHashes);
        }
        effects
    }

    /// What the measure worker found: taken when it is for the window's request. `true` when
    /// the window changed.
    pub fn measure_news(&mut self, news: MeasureNews) -> bool {
        let Some(properties) = self.properties.shown.as_mut() else {
            return false;
        };
        match news {
            MeasureNews::Stated {
                generation,
                item,
                result,
            } if generation == properties.count.generation => {
                let Some(shown) = properties.items.get_mut(item) else {
                    return false;
                };
                match result {
                    Ok(entry) => shown.entry = Some(entry),
                    Err(kind) => shown.unreadable = Some(kind),
                }
                properties.recount();
            }
            MeasureNews::Linked {
                generation,
                item,
                result,
            } if generation == properties.count.generation => {
                let Some(shown) = properties.items.get_mut(item) else {
                    return false;
                };
                shown.link = Some(result);
            }
            MeasureNews::Unlisted {
                generation,
                item,
                kind,
            } if generation == properties.count.generation => {
                let Some(shown) = properties.items.get_mut(item) else {
                    return false;
                };
                shown.unlisted = Some(kind);
            }
            MeasureNews::Counted { generation, tally }
                if generation == properties.count.generation
                    && properties.count.status == CountStatus::Counting =>
            {
                properties.count.tally = tally;
            }
            _ => return false,
        }
        properties.revision += 1;
        true
    }

    /// The measure worker ended the request with `generation`: with the whole count, or
    /// failed (`None`: it panicked, or no thread could be started). `true` when the window
    /// changed.
    pub fn measured(&mut self, generation: u64, tally: Option<Tally>) -> bool {
        let Some(properties) = self.properties.shown.as_mut() else {
            return false;
        };
        if generation != properties.count.generation || !properties.measuring {
            return false;
        }
        properties.measuring = false;
        if properties.count.status == CountStatus::Counting {
            match tally {
                Some(tally) => {
                    properties.count.tally = tally;
                    properties.count.status = CountStatus::Done;
                }
                None => properties.count.status = CountStatus::Failed,
            }
        }
        // What the worker did not get to read: the folder a pane shows, and where a lone link
        // leads (a worker that failed told neither).
        let lone = properties.items.len() == 1;
        for item in &mut properties.items {
            if item.entry.is_none() && item.unreadable.is_none() {
                item.unreadable = Some(ErrorKind::Other);
            }
            let link = item
                .entry
                .as_ref()
                .is_some_and(|entry| entry.kind == EntryKind::Symlink);
            if lone && link && item.link.is_none() {
                item.link = Some(Err(ErrorKind::Other));
            }
        }
        properties.revision += 1;
        true
    }

    /// 중지: the count stops where it is; the window shows what it found.
    pub fn stop_count(&mut self) -> Vec<Effect> {
        let Some(properties) = self.properties.shown.as_mut() else {
            return Vec::new();
        };
        if properties.count.status != CountStatus::Counting {
            return Vec::new();
        }
        properties.count.status = CountStatus::Stopped;
        properties.measuring = false;
        for item in &mut properties.items {
            if item.entry.is_none() && item.unreadable.is_none() {
                item.unreadable = Some(ErrorKind::Cancelled);
            }
        }
        properties.revision += 1;
        vec![Effect::StopMeasure]
    }

    /// The algorithm numbered `number` was chosen: the files are hashed with it, under a new
    /// generation (the request replaces the one before on the worker, which stops). The same
    /// algorithm again makes the checksums 중지 stopped, and changes nothing else.
    pub fn choose_algorithm(&mut self, number: i32) -> Vec<Effect> {
        let Some(algorithm) = Algorithm::from_number(number) else {
            return Vec::new();
        };
        let Some(properties) = self.properties.shown.as_mut() else {
            return Vec::new();
        };
        let hashing = &mut properties.hashing;
        let again = hashing.algorithm == Some(algorithm);
        let stopped = hashing
            .rows
            .iter()
            .any(|row| row.status == HashStatus::Stopped);
        if hashing.rows.is_empty() || (again && !stopped) {
            return Vec::new();
        }
        let under_way = hashing.under_way();
        hashing.algorithm = Some(algorithm);
        hashing.generation = next_generation();
        let mut files = Vec::new();
        for (index, row) in hashing.rows.iter_mut().enumerate() {
            // Again, only what 중지 left; another algorithm, every file but those online only.
            let wanted = if again {
                row.status == HashStatus::Stopped
            } else {
                row.status != HashStatus::OnlineOnly
            };
            if !wanted {
                continue;
            }
            row.status = HashStatus::Waiting;
            files.push((index, properties.items[row.item].path.clone()));
        }
        if !again {
            properties.copied = None;
            self.properties.copying = None;
        }
        properties.copy_problem = None;
        properties.revision += 1;
        if files.is_empty() {
            return if under_way {
                vec![Effect::StopHashes]
            } else {
                Vec::new()
            };
        }
        vec![Effect::Hash(HashRequest {
            generation: hashing.generation,
            algorithm,
            files,
        })]
    }

    /// 중지 of the checksums: the rows still to be made stop and say so, the ones made stay;
    /// the same algorithm chosen again makes the rest.
    pub fn stop_hashes(&mut self) -> Vec<Effect> {
        let Some(properties) = self.properties.shown.as_mut() else {
            return Vec::new();
        };
        if !properties.hashing.under_way() {
            return Vec::new();
        }
        for row in &mut properties.hashing.rows {
            if !row.status.finished() {
                row.status = HashStatus::Stopped;
            }
        }
        properties.revision += 1;
        vec![Effect::StopHashes]
    }

    /// What the worker of the checksums told: taken when it is for the window's request, and
    /// for the rows still to finish. `true` when the window changed.
    pub fn hash_news(&mut self, news: HashNews) -> bool {
        let Some(properties) = self.properties.shown.as_mut() else {
            return false;
        };
        let (generation, statuses) = match news {
            HashNews::Reading {
                generation,
                row,
                done,
                total,
            } => (generation, vec![(row, HashStatus::Reading { done, total })]),
            HashNews::Hashed {
                generation,
                results,
            } => (
                generation,
                results
                    .into_iter()
                    .map(|(row, result)| {
                        let status = match result {
                            Ok(digest) => HashStatus::Done(digest),
                            Err(failure) => HashStatus::Failed(failure),
                        };
                        (row, status)
                    })
                    .collect(),
            ),
        };
        if generation != properties.hashing.generation || properties.hashing.algorithm.is_none() {
            return false;
        }
        let mut changed = false;
        for (row, status) in statuses {
            if let Some(shown) = properties.hashing.rows.get_mut(row)
                && !shown.status.finished()
            {
                shown.status = status;
                changed = true;
            }
        }
        if changed {
            properties.revision += 1;
        }
        changed
    }

    /// The worker of the checksums could not make those of the request with `generation` (it
    /// panicked, or no thread could be started): the rows still to finish say so. `true` when
    /// the window changed.
    pub fn hashes_failed(&mut self, generation: u64) -> bool {
        let Some(properties) = self.properties.shown.as_mut() else {
            return false;
        };
        if generation != properties.hashing.generation || properties.hashing.algorithm.is_none() {
            return false;
        }
        let mut changed = false;
        for row in &mut properties.hashing.rows {
            if !row.status.finished() {
                row.status =
                    HashStatus::Failed(crate::hashing::HashFailure::Unreadable(ErrorKind::Other));
                changed = true;
            }
        }
        if changed {
            properties.revision += 1;
        }
        changed
    }

    /// The field 비교 holds `text` now (committed: an input method's syllable comes once it is).
    pub fn compare_edited(&mut self, text: &str) {
        if let Some(properties) = self.properties.shown.as_mut()
            && properties.compare != text
        {
            text.clone_into(&mut properties.compare);
            properties.revision += 1;
        }
    }

    /// 복사 of row `row`: its checksum goes on the clipboard as text, once it is made.
    pub fn copy_checksum(&mut self, row: usize) -> Vec<Effect> {
        let Some(properties) = self.properties.shown.as_mut() else {
            return Vec::new();
        };
        let Some(HashStatus::Done(digest)) =
            properties.hashing.rows.get(row).map(|row| &row.status)
        else {
            return Vec::new();
        };
        let text = digest.clone();
        self.properties.token += 1;
        let token = self.properties.token;
        self.properties.copying = Some((token, row));
        properties.copy_problem = None;
        properties.revision += 1;
        vec![Effect::Clipboard(ClipboardWork::PutText { token, text })]
    }

    /// How the copy `token` of a checksum went: its row says 복사함, or the window says why
    /// not. Nothing for a copy that a later one, another algorithm or the window's end
    /// replaced.
    pub fn checksum_copied(
        &mut self,
        token: u64,
        result: Result<(), ClipboardFailure>,
    ) -> Vec<Effect> {
        let Some((asked, row)) = self.properties.copying else {
            return Vec::new();
        };
        let Some(properties) = self.properties.shown.as_mut() else {
            return Vec::new();
        };
        if asked != token {
            return Vec::new();
        }
        self.properties.copying = None;
        match result {
            Ok(()) => properties.copied = Some(row),
            Err(failure) => {
                properties.copied = None;
                properties.copy_problem = Some(failure);
            }
        }
        properties.revision += 1;
        Vec::new()
    }
}
