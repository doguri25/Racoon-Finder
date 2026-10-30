//! What the properties window (OPS-06's own window) shows of the items it opened for, and the
//! work a worker thread does to find out what their folder's listing does not tell
//! ([`measure`]).
//!
//! The window opens for the items a command acts on in the pane with the keys (the selected
//! rows, or the row the user put the cursor on), or, with none, for the folder the pane shows,
//! as File Explorer's and Finder's do. It shows them as they were listed: their kind, place,
//! size, times and attributes. A worker adds the rest, with the generation of the window's
//! request ([`MeasureRequest`]), which the state compares before it takes anything:
//!
//! - the entry of the folder the pane shows (the window opened with nothing chosen), which no
//!   listing gave;
//! - where a link leads, when the window shows the link alone;
//! - how much the folders hold: every file and folder under them, counted as the listings go
//!   ([`Tally`]), told every [`PROGRESS_GAP`](crate::hashing::PROGRESS_GAP) at most.
//!
//! The count enters only folders (`EntryKind::Dir`): a link, a Windows junction or a mounted
//! volume on Windows (both reparse points) is counted as an item and never followed, so a link
//! back to a parent cannot make it go round. It stays on the file system each folder it starts
//! from is on, as `du -x` does: a folder that is another volume's (a drive or a network share
//! mounted inside it, `/proc` under `/`) holds what is not this folder's to take room, and can
//! be huge or never end; such folders are counted apart ([`Tally::elsewhere`]) and said so. A
//! folder whose identity it has seen already (a bind mount of a parent) is not entered twice.
//! A file with several names (hard links) counts once for each name, as in File Explorer and
//! unlike `du`, which counts its blocks once. It looks at its [`CancelToken`] between the
//! listings' batches (2,000 items at most), so 중지, the window's end and a newer request stop it
//! at once, even in a folder of a million files.
//!
//! A size is whole only when the count ended and read every folder: one that stopped, failed or
//! met a folder it could not list says `이상` (at least) after it. A folder of the items that
//! cannot be listed itself ([`MeasureNews::Unlisted`]), or the folder a pane shows that cannot
//! be read, says why in place of its size, rather than show an empty folder.

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime};

use jiff::tz::TimeZone;
use rf_core::{Attributes, Entry, EntryKind, ErrorKind, FileId, VPath, VolumeId};
use rf_vfs::{CancelToken, Vfs};

use crate::format::{format_time_in, kind_label, short_path};
use crate::hashing::{Algorithm, HashFailure, same_digest};
use crate::state::ClipboardFailure;
use crate::words;

/// How many characters a path of the window's facts (its place, a link's target) takes at most:
/// the middle of a longer one is left out ([`short_path`]), so that its end stays in view on one
/// line.
pub const PLACE_CHARS: usize = 56;

/// The most files the window hashes: the first ones of the items, in the order of the rows.
/// Hashing more is a batch job, and a table of more is no help to read.
pub const MAX_HASHED_FILES: usize = 1_000;

/// What the window asks a worker to find out, under `generation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasureRequest {
    pub generation: u64,
    pub items: Vec<MeasureItem>,
    /// Read where each link among the items leads (the window shows one item).
    pub link_targets: bool,
}

/// An item of a [`MeasureRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasureItem {
    pub path: VPath,
    /// The item as its folder listed it; `None` for the folder a pane shows, which the worker
    /// reads first.
    pub entry: Option<Entry>,
}

/// What the worker tells of a [`MeasureRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeasureNews {
    /// Item `item` (one without an entry) is this, or could not be read.
    Stated {
        generation: u64,
        item: usize,
        result: Result<Entry, ErrorKind>,
    },
    /// Link `item` leads here (the target as the link stores it), or could not be read.
    Linked {
        generation: u64,
        item: usize,
        result: Result<String, ErrorKind>,
    },
    /// Folder `item`, one of the items, could not be listed: what it holds is not counted.
    Unlisted {
        generation: u64,
        item: usize,
        kind: ErrorKind,
    },
    /// What the count has found so far.
    Counted { generation: u64, tally: Tally },
}

/// What the folders of a [`MeasureRequest`] hold, everything under them (not the folders
/// themselves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// The items that are no folder: files, links, special files.
    pub files: u64,
    pub folders: u64,
    /// The size of the files.
    pub bytes: u64,
    /// The folders that could not be listed, the items' own among them: what they hold is not
    /// counted.
    pub unreadable: u64,
    /// The folders on another file system, which the count does not enter.
    pub elsewhere: u64,
}

/// Finds out what `request` asks through `vfs`, telling `tell` what it finds: the entries it
/// reads, the links' targets, and the count so far (at most every `gap`; the whole count is what
/// it returns). Blocks: call it on a worker thread. The count it ends with, or `None` once
/// `cancel` stopped it.
pub fn measure(
    request: &MeasureRequest,
    vfs: &Vfs,
    cancel: &CancelToken,
    gap: Duration,
    tell: &mut dyn FnMut(MeasureNews),
) -> Option<Tally> {
    let generation = request.generation;
    let mut entries: Vec<Option<Entry>> = Vec::with_capacity(request.items.len());
    for (index, item) in request.items.iter().enumerate() {
        if cancel.is_cancelled() {
            return None;
        }
        let entry = if let Some(entry) = &item.entry {
            Some(entry.clone())
        } else {
            let result = vfs
                .provider(&item.path)
                .and_then(|provider| provider.stat(&item.path))
                .map_err(|error| error.kind());
            let entry = result.as_ref().ok().cloned();
            tell(MeasureNews::Stated {
                generation,
                item: index,
                result,
            });
            entry
        };
        let link = entry
            .as_ref()
            .is_some_and(|entry| entry.kind == EntryKind::Symlink);
        if request.link_targets && link {
            let result = vfs
                .provider(&item.path)
                .and_then(|provider| provider.read_link(&item.path))
                .map(|link| link.target.to_string_lossy().into_owned())
                .map_err(|error| error.kind());
            tell(MeasureNews::Linked {
                generation,
                item: index,
                result,
            });
        }
        entries.push(entry);
    }
    let mut count = Walk::new(gap);
    for (index, (item, entry)) in request.items.iter().zip(&entries).enumerate() {
        if let Some(entry) = entry.as_ref().filter(|entry| entry.kind == EntryKind::Dir) {
            count.start(index, item.path.clone(), entry);
        }
    }
    while let Some((dir, volume, item)) = count.pending.pop() {
        if cancel.is_cancelled() {
            return None;
        }
        let listed = match vfs.provider(&dir) {
            Ok(provider) => provider.list(&dir, cancel, &mut |batch| {
                count.take(&dir, volume, batch);
                count.report(generation, tell);
            }),
            Err(error) => Err(error),
        };
        match listed {
            Ok(()) => {}
            Err(_) if cancel.is_cancelled() => return None,
            // What it holds is not counted, and the window says so.
            Err(error) => {
                count.tally.unreadable += 1;
                if let Some(item) = item {
                    tell(MeasureNews::Unlisted {
                        generation,
                        item,
                        kind: error.kind(),
                    });
                }
            }
        }
        count.report(generation, tell);
    }
    Some(count.tally)
}

/// A count under way: what it found, the folders it is still to list (each with the volume of
/// the folder it started from, and the number of the item for an item's own folder), and the
/// folders it has entered.
struct Walk {
    tally: Tally,
    pending: Vec<(VPath, Option<VolumeId>, Option<usize>)>,
    /// The folders entered, by identity (where the listing gives one): none twice.
    entered: HashSet<(VolumeId, FileId)>,
    /// When the count was last told, and how often it is.
    told: Instant,
    gap: Duration,
}

impl Walk {
    fn new(gap: Duration) -> Walk {
        Walk {
            tally: Tally::default(),
            pending: Vec::new(),
            entered: HashSet::new(),
            told: Instant::now(),
            gap,
        }
    }

    /// The folder `entry` at `path`, item `item`, is to be listed.
    fn start(&mut self, item: usize, path: VPath, entry: &Entry) {
        if let Some(identity) = entry.identity()
            && !self.entered.insert(identity)
        {
            return;
        }
        self.pending.push((path, entry.volume_id, Some(item)));
    }

    /// Counts `batch`, a part of the listing of `dir`, a folder on `volume`: its files, and its
    /// folders, which are listed later, unless they are on another volume or entered already.
    fn take(&mut self, dir: &VPath, volume: Option<VolumeId>, batch: Vec<Entry>) {
        for entry in batch {
            if entry.kind != EntryKind::Dir {
                self.tally.files += 1;
                if entry.kind == EntryKind::File {
                    self.tally.bytes = self.tally.bytes.saturating_add(entry.size);
                }
                continue;
            }
            let elsewhere = matches!(
                (volume, entry.volume_id),
                (Some(from), Some(here)) if from != here
            );
            if elsewhere {
                self.tally.elsewhere += 1;
                continue;
            }
            if let Some(identity) = entry.identity()
                && !self.entered.insert(identity)
            {
                // A folder seen already: a bind mount of a parent, or of another item.
                continue;
            }
            self.tally.folders += 1;
            if let Ok(path) = dir.join(&entry.name) {
                self.pending.push((path, volume, None));
            }
        }
    }

    /// Tells the count so far, when `gap` has passed since it was last told.
    fn report(&mut self, generation: u64, tell: &mut dyn FnMut(MeasureNews)) {
        if self.told.elapsed() >= self.gap {
            self.told = Instant::now();
            tell(MeasureNews::Counted {
                generation,
                tally: self.tally,
            });
        }
    }
}

/// The properties window: the items it opened for, and what it has found out of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Properties {
    /// Names this window: a new one for every window.
    pub id: u64,
    /// Changes with every change of what the window shows, which is made anew only then.
    pub revision: u64,
    /// The folder the items are in: the pane's.
    pub folder: VPath,
    /// Nothing was chosen: the window is for the folder the pane shows, its one item.
    pub own_folder: bool,
    pub items: Vec<Item>,
    /// What the chosen items are, as the listing gave them.
    pub chosen: Chosen,
    pub count: Count,
    /// The measure worker has the window's request (it reads, or counts) and has not ended it.
    pub measuring: bool,
    pub hashing: Hashing,
    /// The hash the user pasted, to compare; as the field holds it, once committed.
    pub compare: String,
    /// The row whose checksum went to the clipboard last.
    pub copied: Option<usize>,
    /// Why the last checksum did not go to the clipboard.
    pub copy_problem: Option<ClipboardFailure>,
}

/// An item of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: VPath,
    /// `None` until a worker has read it (the folder the pane shows).
    pub entry: Option<Entry>,
    /// Why the worker could not read it.
    pub unreadable: Option<ErrorKind>,
    /// Why the worker could not list it, a folder: what it holds is not counted.
    pub unlisted: Option<ErrorKind>,
    /// Where a link leads, once a worker read it.
    pub link: Option<Result<String, ErrorKind>>,
}

/// How many of the chosen items are what.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chosen {
    /// The items that are no folder.
    pub files: u64,
    pub folders: u64,
    /// The size of the files among them.
    pub bytes: u64,
    /// How many of them are read-only, hidden, system and archive, in that order.
    pub flagged: [usize; 4],
}

impl Chosen {
    /// What `items` are, as far as their entries are known.
    pub fn of(items: &[Item]) -> Chosen {
        let mut chosen = Chosen::default();
        for entry in items.iter().filter_map(|item| item.entry.as_ref()) {
            if entry.kind == EntryKind::Dir {
                chosen.folders += 1;
            } else {
                chosen.files += 1;
            }
            if entry.kind == EntryKind::File {
                chosen.bytes = chosen.bytes.saturating_add(entry.size);
            }
            for (flagged, attribute) in chosen.flagged.iter_mut().zip(SHOWN_ATTRIBUTES) {
                *flagged += usize::from(entry.attributes.contains(attribute));
            }
        }
        chosen
    }
}

/// The attributes the window shows, in its order.
pub const SHOWN_ATTRIBUTES: [Attributes; 4] = [
    Attributes::READONLY,
    Attributes::HIDDEN,
    Attributes::SYSTEM,
    Attributes::ARCHIVE,
];

/// The count of what the folders hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count {
    /// The generation of the window's request to the worker.
    pub generation: u64,
    pub tally: Tally,
    pub status: CountStatus,
}

/// Where the count stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CountStatus {
    /// No folder to count.
    #[default]
    None,
    Counting,
    Done,
    /// 중지 stopped it: the tally is what it had found.
    Stopped,
    /// The worker failed (it panicked, or no thread could be started).
    Failed,
}

/// The checksums of the window's files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hashing {
    /// The files, as rows: the first [`MAX_HASHED_FILES`] of them.
    pub rows: Vec<HashRow>,
    /// The files left out past [`MAX_HASHED_FILES`].
    pub left_out: usize,
    /// The algorithm chosen; `None` until one is.
    pub algorithm: Option<Algorithm>,
    /// The generation of the request for the rows.
    pub generation: u64,
}

/// A file to hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashRow {
    /// Its item.
    pub item: usize,
    pub status: HashStatus,
}

/// Where a file's checksum stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HashStatus {
    /// No algorithm is chosen yet.
    Idle,
    /// It waits for the files before it.
    Waiting,
    /// It is read: `done` of `total` bytes.
    Reading {
        done: u64,
        total: u64,
    },
    Done(String),
    Failed(HashFailure),
    /// A file stored online only (`Attributes::OFFLINE`), which reading would download: it is
    /// not read, as the preview does not read it.
    OnlineOnly,
    /// 중지 stopped it before it was made: the same algorithm chosen again makes it.
    Stopped,
}

impl HashStatus {
    /// Whether nothing more comes for it.
    pub fn finished(&self) -> bool {
        !matches!(self, HashStatus::Waiting | HashStatus::Reading { .. })
    }
}

impl Hashing {
    /// Whether the worker is making checksums of the rows: some wait or are read.
    pub fn under_way(&self) -> bool {
        self.algorithm.is_some() && self.rows.iter().any(|row| !row.status.finished())
    }
}

impl Properties {
    /// The window `id` for `items` in `folder`, or for `folder` itself (`own_folder`, its one
    /// item without an entry). Its files are rows to hash, each waiting for an algorithm.
    pub fn new(id: u64, folder: VPath, own_folder: bool, items: Vec<Item>) -> Properties {
        let chosen = Chosen::of(&items);
        let files: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.entry
                    .as_ref()
                    .is_some_and(|entry| entry.kind == EntryKind::File)
            })
            .map(|(index, _)| index)
            .collect();
        let left_out = files.len().saturating_sub(MAX_HASHED_FILES);
        let rows = files
            .into_iter()
            .take(MAX_HASHED_FILES)
            .map(|item| HashRow {
                item,
                status: HashStatus::Idle,
            })
            .collect();
        Properties {
            id,
            revision: 0,
            folder,
            own_folder,
            items,
            chosen,
            count: Count::default(),
            hashing: Hashing {
                rows,
                left_out,
                ..Hashing::default()
            },
            measuring: false,
            compare: String::new(),
            copied: None,
            copy_problem: None,
        }
    }

    /// Counts the chosen items again, once a worker has read one (the folder a pane shows).
    pub fn recount(&mut self) {
        self.chosen = Chosen::of(&self.items);
    }

    /// Whether the window shows one item.
    pub fn single(&self) -> Option<&Item> {
        match self.items.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// The name of item `index`.
    pub fn name(&self, index: usize) -> String {
        let Some(item) = self.items.get(index) else {
            return String::new();
        };
        match (&item.entry, item.path.file_name()) {
            (Some(entry), _) => entry.name.to_string_lossy().into_owned(),
            (None, Some(name)) => name.to_string_lossy().into_owned(),
            // A root: its whole path.
            (None, None) => item.path.to_string(),
        }
    }
}

/// A line of the window's facts: what it is, and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub label: &'static str,
    pub value: String,
    /// The whole value, when `value` leaves part of it out (a long path): for a tooltip and
    /// screen readers.
    pub full: Option<String>,
}

/// The facts of the window, in their order, with the times in `tz`: for one item, its kind,
/// place, size (and what a folder holds), where a link leads, its times and attributes; for
/// several, how many of what, their folder, their size with what their folders hold, and the
/// attributes they have.
///
/// 접근한 날짜 is what the file system keeps, which may lag: Linux mounts with `relatime` or
/// `noatime` update it seldom or never, Windows can have its last-access updates off, and
/// reading a file (to hash it, for one) can move it.
pub fn facts(properties: &Properties, tz: &TimeZone) -> Vec<Fact> {
    let fact = |label: &'static str, value: String| Fact {
        label,
        value,
        full: None,
    };
    let mut facts = Vec::new();
    let Some(item) = properties.single() else {
        let chosen = &properties.chosen;
        facts.push(fact(
            words::fact_kind(),
            words::chosen_kinds(chosen.files, chosen.folders),
        ));
        facts.push(place_fact(
            words::fact_place(),
            &properties.folder.to_string(),
        ));
        facts.push(fact(words::fact_size(), size_value(properties)));
        if chosen.folders > 0 {
            facts.extend(contents(properties).map(|value| fact(words::fact_contents(), value)));
        }
        let count = properties.items.len();
        facts.push(fact(
            words::fact_attributes(),
            words::attribute_names(&chosen.flagged, count),
        ));
        return facts;
    };
    let unread = || {
        item.unreadable.map_or_else(
            || words::reading().to_owned(),
            |kind| words::error_kind(kind).to_owned(),
        )
    };
    // The folder a pane shows is a folder before the worker has read it.
    let kind = item
        .entry
        .as_ref()
        .map_or(EntryKind::Dir, |entry| entry.kind);
    let kind_name = item.entry.as_ref().map_or_else(
        || kind_label(&Entry::new(properties.name(0), EntryKind::Dir)),
        kind_label,
    );
    facts.push(fact(words::fact_kind(), kind_name));
    if let Some(parent) = item.path.parent() {
        facts.push(place_fact(words::fact_place(), &parent.to_string()));
    }
    // A folder that could not be read or listed says why, not that it is empty.
    let size = match (&item.entry, item.unlisted) {
        (None, _) => Some(unread()),
        (Some(_), Some(kind)) => Some(words::folder_unreadable(kind)),
        (Some(_), None) => None,
    };
    match kind {
        EntryKind::File | EntryKind::Dir => {
            let value = size.clone().unwrap_or_else(|| size_value(properties));
            facts.push(fact(words::fact_size(), value));
        }
        EntryKind::Symlink | EntryKind::Other => {}
    }
    if kind == EntryKind::Dir && size.is_none() {
        facts.extend(contents(properties).map(|value| fact(words::fact_contents(), value)));
    }
    if kind == EntryKind::Symlink {
        facts.push(match &item.link {
            Some(Ok(target)) => place_fact(words::fact_link_target(), target),
            Some(Err(kind)) => fact(
                words::fact_link_target(),
                words::error_kind(*kind).to_owned(),
            ),
            None => fact(words::fact_link_target(), words::reading().to_owned()),
        });
    }
    let time = |time: fn(&Entry) -> Option<SystemTime>| match &item.entry {
        Some(entry) => time(entry).map_or_else(
            || words::unknown_time().to_owned(),
            |time| format_time_in(time, tz),
        ),
        None => unread(),
    };
    facts.push(fact(words::fact_created(), time(|entry| entry.created)));
    facts.push(fact(words::fact_modified(), time(|entry| entry.modified)));
    facts.push(fact(words::fact_accessed(), time(|entry| entry.accessed)));
    let attributes = match &item.entry {
        Some(entry) => {
            let flagged =
                SHOWN_ATTRIBUTES.map(|attribute| usize::from(entry.attributes.contains(attribute)));
            words::attribute_names(&flagged, 1)
        }
        None => unread(),
    };
    facts.push(fact(words::fact_attributes(), attributes));
    facts
}

/// `path` as a fact shows it: on one line, its middle left out when it is long.
fn place(path: &str) -> String {
    short_path(path, PLACE_CHARS)
}

/// The fact `label` of `path`, shortened ([`place`]) with the whole path beside it.
fn place_fact(label: &'static str, path: &str) -> Fact {
    let value = place(path);
    let full = (value != path).then(|| path.to_owned());
    Fact { label, value, full }
}

/// The size the window shows: the files chosen and what the folders hold so far, `이상` (at
/// least) while the count has not ended whole.
fn size_value(properties: &Properties) -> String {
    let bytes = properties
        .chosen
        .bytes
        .saturating_add(properties.count.tally.bytes);
    let whole = matches!(
        properties.count.status,
        CountStatus::None | CountStatus::Done
    ) && properties.count.tally.unreadable == 0;
    words::size_fact(bytes, whole)
}

/// What the folders hold, once the count has found anything (or ended).
fn contents(properties: &Properties) -> Option<String> {
    let count = &properties.count;
    (count.status != CountStatus::None)
        .then(|| words::contents(count.tally.files, count.tally.folders))
}

/// What the line under the facts says of the count: that it goes on, that it stopped or
/// failed, and what it left out; empty when there is nothing to say.
pub fn count_note(properties: &Properties) -> String {
    let count = &properties.count;
    let mut parts = Vec::new();
    match count.status {
        CountStatus::None | CountStatus::Done => {}
        CountStatus::Counting => parts.push(words::counting().to_owned()),
        CountStatus::Stopped => parts.push(words::count_stopped().to_owned()),
        CountStatus::Failed => parts.push(words::count_failed().to_owned()),
    }
    // A lone folder that could not be listed says so in place of its size.
    let lone_unlisted = properties
        .single()
        .is_some_and(|item| item.unlisted.is_some());
    if count.tally.unreadable > 0 && !lone_unlisted {
        parts.push(words::unreadable_folders(count.tally.unreadable));
    }
    if count.tally.elsewhere > 0 {
        parts.push(words::folders_elsewhere(count.tally.elsewhere));
    }
    // A line each: the window lays the lines out as they are, without wrapping them.
    parts.join("\n")
}

/// A row of the checksums as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct HashLine {
    pub name: String,
    /// The checksum, or what the row waits for or why it has none.
    pub value: String,
    /// How far the file is read, 0 to 1, while it is read.
    pub progress: Option<f32>,
    /// `value` is a checksum.
    pub done: bool,
    pub copied: bool,
    /// The pasted hash is this checksum.
    pub matched: bool,
}

/// The rows of the checksums.
pub fn hash_lines(properties: &Properties) -> Vec<HashLine> {
    let compare = properties.compare.as_str();
    properties
        .hashing
        .rows
        .iter()
        .enumerate()
        .map(|(row, hash)| {
            let (value, progress) = match &hash.status {
                HashStatus::Idle => (String::new(), None),
                HashStatus::Waiting => (words::hash_waiting().to_owned(), None),
                HashStatus::Reading { done, total } => {
                    let fraction = fraction(*done, *total);
                    (words::hash_percent(fraction), Some(fraction))
                }
                HashStatus::Done(digest) => (digest.clone(), None),
                HashStatus::Failed(failure) => (words::hash_failure(*failure), None),
                HashStatus::OnlineOnly => (words::hash_online_only().to_owned(), None),
                HashStatus::Stopped => (words::hash_stopped().to_owned(), None),
            };
            let done = matches!(hash.status, HashStatus::Done(_));
            let matched = match &hash.status {
                HashStatus::Done(digest) => same_digest(compare, digest) == Some(true),
                _ => false,
            };
            HashLine {
                name: properties.name(hash.item),
                value,
                progress,
                done,
                copied: properties.copied == Some(row),
                matched,
            }
        })
        .collect()
}

/// `done` of `total`, from 0 to 1 (0 of nothing), for a bar and a percentage: the precision a
/// float loses of large counts is not seen there.
#[allow(clippy::cast_precision_loss, reason = "a fraction for a bar")]
fn fraction(done: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    }
}

/// What the line under the checksums says: what to do first, how far several files are, what
/// was left out, and why a copy failed; empty when there is nothing to say.
pub fn hash_note(properties: &Properties) -> String {
    let hashing = &properties.hashing;
    if hashing.rows.is_empty() {
        return words::only_files_hashed().to_owned();
    }
    let mut parts = Vec::new();
    if hashing.algorithm.is_none() {
        parts.push(words::choose_algorithm().to_owned());
    } else if hashing.under_way() {
        if hashing.rows.len() > 1 {
            let finished = hashing
                .rows
                .iter()
                .filter(|row| row.status.finished())
                .count();
            parts.push(words::hashing_files(finished, hashing.rows.len()));
        }
    } else if hashing
        .rows
        .iter()
        .any(|row| row.status == HashStatus::Stopped)
    {
        parts.push(words::hashes_stopped().to_owned());
    }
    if hashing.left_out > 0 {
        parts.push(words::hashed_first(MAX_HASHED_FILES));
    }
    if let Some(problem) = properties.copy_problem {
        parts.push(words::clipboard_failed(problem).to_owned());
    }
    // A line each, as in [`count_note`].
    parts.join("\n")
}

/// What the comparison with the pasted hash says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to say: no hash pasted, or no checksum yet.
    None,
    /// A checksum is the pasted hash: of the file with this name, or of this many files.
    Match(Option<String>, usize),
    /// No checksum is, and none is still to come.
    Mismatch,
    /// None is so far, and some are still to come.
    Waiting,
}

/// What the comparison with the pasted hash says now, given the rows as [`hash_lines`] made them
/// (`lines`, made once for the window's view and for this).
pub fn verdict(properties: &Properties, lines: &[HashLine]) -> Verdict {
    let hashing = &properties.hashing;
    if hashing.algorithm.is_none() || properties.compare.trim().is_empty() {
        return Verdict::None;
    }
    let matched: Vec<&HashLine> = lines.iter().filter(|line| line.matched).collect();
    if let Some(first) = matched.first() {
        let name = (hashing.rows.len() > 1 && matched.len() == 1).then(|| first.name.clone());
        return Verdict::Match(name, matched.len());
    }
    // A row 중지 stopped may still be made: no mismatch is told before it is.
    let stopped = hashing
        .rows
        .iter()
        .any(|row| row.status == HashStatus::Stopped);
    if !stopped && hashing.rows.iter().all(|row| row.status.finished()) {
        if lines.iter().any(|line| line.done) {
            Verdict::Mismatch
        } else {
            Verdict::None
        }
    } else {
        Verdict::Waiting
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rf_core::{Error, Scheme};
    use rf_vfs::{
        Capabilities, DeleteMode, Deleted, DiskSpace, LinkTarget, Provider, ReadStream, RenameMode,
        WatchHandle, WatchSink, WriteMode, WriteStream,
    };

    use super::*;

    /// A tree of folders in memory, listed as the local provider lists: each folder's entries
    /// with the volume and file they are on. A folder named `locked` cannot be listed.
    #[derive(Debug, Default)]
    struct Memory {
        folders: Vec<(VPath, Vec<Entry>)>,
        stats: Vec<(VPath, Entry)>,
        links: Vec<(VPath, std::path::PathBuf)>,
        /// The folders listed, in order.
        listed: Mutex<Vec<VPath>>,
    }

    impl Memory {
        fn folder(&mut self, path: &VPath, entries: Vec<Entry>) {
            self.folders.push((path.clone(), entries));
        }
    }

    fn unused(op: &str) -> Error {
        Error::Unsupported(format!("{op} is not used here"))
    }

    impl Provider for Memory {
        fn scheme(&self) -> Scheme {
            Scheme::Local
        }

        fn capabilities(&self, _: &VPath) -> Capabilities {
            Capabilities::empty()
        }

        fn list(
            &self,
            dir: &VPath,
            cancel: &CancelToken,
            sink: &mut dyn FnMut(Vec<Entry>),
        ) -> rf_core::Result<()> {
            cancel.check()?;
            self.listed.lock().expect("one lister").push(dir.clone());
            if dir.file_name() == Some(std::ffi::OsStr::new("locked")) {
                let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
                return Err(Error::io("read_dir", dir.clone(), denied));
            }
            let (_, entries) = self
                .folders
                .iter()
                .find(|(path, _)| path == dir)
                .ok_or_else(|| {
                    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
                    Error::io("read_dir", dir.clone(), missing)
                })?;
            // Two a batch, so that a cancel between batches shows.
            for batch in entries.chunks(2) {
                cancel.check()?;
                sink(batch.to_vec());
            }
            Ok(())
        }

        fn stat(&self, path: &VPath) -> rf_core::Result<Entry> {
            self.stats
                .iter()
                .find(|(known, _)| known == path)
                .map(|(_, entry)| entry.clone())
                .ok_or_else(|| {
                    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
                    Error::io("stat", path.clone(), missing)
                })
        }

        fn read_link(&self, path: &VPath) -> rf_core::Result<LinkTarget> {
            self.links
                .iter()
                .find(|(known, _)| known == path)
                .map(|(_, target)| LinkTarget {
                    target: target.clone(),
                    is_dir: false,
                })
                .ok_or_else(|| unused("read_link"))
        }

        fn create_dir(&self, _: &VPath) -> rf_core::Result<()> {
            Err(unused("create_dir"))
        }

        fn create_file(&self, _: &VPath) -> rf_core::Result<()> {
            Err(unused("create_file"))
        }

        fn rename(&self, _: &VPath, _: &VPath, _: RenameMode) -> rf_core::Result<()> {
            Err(unused("rename"))
        }

        fn delete(&self, _: &VPath, _: DeleteMode) -> rf_core::Result<Deleted> {
            Err(unused("delete"))
        }

        fn open_read(&self, _: &VPath) -> rf_core::Result<Box<dyn ReadStream>> {
            Err(unused("open_read"))
        }

        fn open_write(&self, _: &VPath, _: WriteMode) -> rf_core::Result<Box<dyn WriteStream>> {
            Err(unused("open_write"))
        }

        fn space(&self, _: &VPath) -> rf_core::Result<Option<DiskSpace>> {
            Ok(None)
        }

        fn watch(&self, _: &VPath, _: WatchSink) -> rf_core::Result<WatchHandle> {
            Err(unused("watch"))
        }
    }

    fn at(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join("memory").join(name)).expect("a local path")
    }

    /// An entry named `name` of `kind`, `size` bytes, on volume `volume` as file `file`.
    fn on(name: &str, kind: EntryKind, size: u64, volume: u64, file: u128) -> Entry {
        let mut entry = Entry::new(name, kind);
        entry.size = size;
        entry.volume_id = Some(VolumeId(volume));
        entry.file_id = Some(FileId(file));
        entry
    }

    /// `root` (volume 1, file 1) holds `a.txt` (3 bytes), `link` (a link, never followed),
    /// `sub` (with `b.bin`, 10 bytes, and `back`, a bind mount of `root` itself), `mnt` (another
    /// volume's root, with a large file) and `locked` (which cannot be listed).
    fn tree() -> Memory {
        let mut memory = Memory::default();
        let root = at("root");
        memory.folder(
            &root,
            vec![
                on("a.txt", EntryKind::File, 3, 1, 10),
                on("link", EntryKind::Symlink, 40, 1, 11),
                on("sub", EntryKind::Dir, 0, 1, 2),
                on("mnt", EntryKind::Dir, 0, 2, 1),
                on("locked", EntryKind::Dir, 0, 1, 3),
            ],
        );
        let sub = root.join("sub").expect("a name");
        memory.folder(
            &sub,
            vec![
                on("b.bin", EntryKind::File, 10, 1, 12),
                on("back", EntryKind::Dir, 0, 1, 1),
            ],
        );
        memory.folder(
            &root.join("mnt").expect("a name"),
            vec![on("big", EntryKind::File, 1 << 40, 2, 5)],
        );
        memory.folder(
            &sub.join("back").expect("a name"),
            vec![on("a.txt", EntryKind::File, 3, 1, 10)],
        );
        memory
    }

    fn vfs_of(memory: Memory) -> (Vfs, Arc<Memory>) {
        let memory = Arc::new(memory);
        let mut vfs = Vfs::new();
        vfs.register(Arc::clone(&memory) as Arc<dyn Provider>);
        (vfs, memory)
    }

    fn request(items: Vec<MeasureItem>, link_targets: bool) -> MeasureRequest {
        MeasureRequest {
            generation: 5,
            items,
            link_targets,
        }
    }

    #[test]
    fn a_count_takes_every_folder_under_the_items_once_and_on_their_own_volume() {
        let (vfs, memory) = vfs_of(tree());
        let items = vec![
            MeasureItem {
                path: at("root"),
                entry: Some(on("root", EntryKind::Dir, 0, 1, 1)),
            },
            MeasureItem {
                path: at("x.txt"),
                entry: Some(on("x.txt", EntryKind::File, 4, 1, 20)),
            },
        ];
        let mut news = Vec::new();
        let tally = measure(
            &request(items, false),
            &vfs,
            &CancelToken::new(),
            Duration::ZERO,
            &mut |item| news.push(item),
        );
        // a.txt, link and b.bin; sub and locked. Not `back` (root again), not what `mnt` holds,
        // and the chosen file is not what the folders hold.
        assert_eq!(
            tally,
            Some(Tally {
                files: 3,
                folders: 2,
                bytes: 13,
                unreadable: 1,
                elsewhere: 1,
            })
        );
        let listed = memory.listed.lock().expect("one lister").clone();
        assert_eq!(listed.len(), 3, "root, sub and locked: {listed:?}");
        // Told as it went, the last time with what it ends with.
        let counted: Vec<Tally> = news
            .iter()
            .filter_map(|item| match item {
                MeasureNews::Counted {
                    generation: 5,
                    tally,
                } => Some(*tally),
                _ => None,
            })
            .collect();
        assert!(counted.len() >= 3, "{counted:?}");
        assert_eq!(counted.last().copied(), tally);
        assert!(
            counted
                .windows(2)
                .all(|pair| pair[0].files <= pair[1].files)
        );
    }

    #[test]
    fn the_folder_a_pane_shows_is_read_first_and_a_lone_link_says_where_it_leads() {
        let mut memory = tree();
        memory
            .stats
            .push((at("root"), on("root", EntryKind::Dir, 0, 1, 1)));
        let link = at("root").join("link").expect("a name");
        memory
            .links
            .push((link.clone(), std::path::PathBuf::from("../elsewhere")));
        let (vfs, _) = vfs_of(memory);
        let mut news = Vec::new();
        let tally = measure(
            &request(
                vec![MeasureItem {
                    path: at("root"),
                    entry: None,
                }],
                true,
            ),
            &vfs,
            &CancelToken::new(),
            Duration::from_secs(3600),
            &mut |item| news.push(item),
        );
        assert_eq!(tally.map(|tally| tally.files), Some(3));
        assert_eq!(
            news.first(),
            Some(&MeasureNews::Stated {
                generation: 5,
                item: 0,
                result: Ok(on("root", EntryKind::Dir, 0, 1, 1)),
            })
        );
        // No progress within an hour's gap: the count comes once, at its end.
        assert_eq!(news.len(), 1, "{news:?}");

        let mut news = Vec::new();
        let (vfs, _) = vfs_of({
            let mut memory = tree();
            memory
                .links
                .push((link.clone(), std::path::PathBuf::from("../elsewhere")));
            memory
        });
        let tally = measure(
            &request(
                vec![MeasureItem {
                    path: link,
                    entry: Some(on("link", EntryKind::Symlink, 40, 1, 11)),
                }],
                true,
            ),
            &vfs,
            &CancelToken::new(),
            Duration::ZERO,
            &mut |item| news.push(item),
        );
        assert_eq!(tally, Some(Tally::default()));
        assert!(
            news.contains(&MeasureNews::Linked {
                generation: 5,
                item: 0,
                result: Ok("../elsewhere".to_owned()),
            }),
            "{news:?}"
        );
    }

    #[test]
    fn a_folder_that_is_gone_says_why() {
        let (vfs, _) = vfs_of(tree());
        let mut news = Vec::new();
        let tally = measure(
            &request(
                vec![MeasureItem {
                    path: at("gone"),
                    entry: None,
                }],
                false,
            ),
            &vfs,
            &CancelToken::new(),
            Duration::ZERO,
            &mut |item| news.push(item),
        );
        assert_eq!(tally, Some(Tally::default()));
        assert_eq!(
            news.first(),
            Some(&MeasureNews::Stated {
                generation: 5,
                item: 0,
                result: Err(ErrorKind::NotFound),
            })
        );
    }

    #[test]
    fn a_cancel_stops_the_count_between_batches() {
        let (vfs, memory) = vfs_of(tree());
        let cancel = CancelToken::new();
        let mut news = Vec::new();
        let tally = measure(
            &request(
                vec![MeasureItem {
                    path: at("root"),
                    entry: Some(on("root", EntryKind::Dir, 0, 1, 1)),
                }],
                false,
            ),
            &vfs,
            &cancel,
            Duration::ZERO,
            &mut |item| {
                // 중지 after the first batch of the first folder.
                cancel.cancel();
                news.push(item);
            },
        );
        assert_eq!(tally, None);
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(memory.listed.lock().expect("one lister").len(), 1);
    }

    #[test]
    fn a_count_on_the_disk_takes_files_and_folders_and_stops_when_cancelled() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub").join("deeper")).expect("folders");
        std::fs::create_dir(root.join("empty")).expect("a folder");
        std::fs::write(root.join("a.txt"), b"abc").expect("a file");
        std::fs::write(root.join("sub").join("b.bin"), [0; 10]).expect("a file");
        std::fs::write(root.join("sub").join("deeper").join("c"), [0; 5]).expect("a file");
        let vfs = Vfs::with_local();
        let path = VPath::local(&root).expect("a local path");
        let entry = vfs
            .provider(&path)
            .and_then(|provider| provider.stat(&path))
            .expect("the folder");
        let items = vec![MeasureItem {
            path,
            entry: Some(entry),
        }];
        let tally = measure(
            &request(items.clone(), false),
            &vfs,
            &CancelToken::new(),
            Duration::ZERO,
            &mut |_| {},
        );
        assert_eq!(
            tally,
            Some(Tally {
                files: 3,
                folders: 3,
                bytes: 18,
                ..Tally::default()
            })
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut news = Vec::new();
        let stopped = measure(
            &request(items, false),
            &vfs,
            &cancel,
            Duration::ZERO,
            &mut |item| news.push(item),
        );
        assert_eq!((stopped, news), (None, Vec::new()));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_back_to_a_parent_is_counted_and_not_followed() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub")).expect("folders");
        std::fs::write(root.join("sub").join("f"), [0; 7]).expect("a file");
        std::os::unix::fs::symlink(&root, root.join("sub").join("up")).expect("a link");
        let vfs = Vfs::with_local();
        let path = VPath::local(&root).expect("a local path");
        let entry = vfs
            .provider(&path)
            .and_then(|provider| provider.stat(&path))
            .expect("the folder");
        let tally = measure(
            &request(
                vec![MeasureItem {
                    path,
                    entry: Some(entry),
                }],
                false,
            ),
            &vfs,
            &CancelToken::new(),
            Duration::ZERO,
            &mut |_| {},
        );
        // `f` and the link `up`; `sub`.
        assert_eq!(
            tally.map(|tally| (tally.files, tally.folders)),
            Some((2, 1))
        );
        assert_eq!(tally.map(|tally| tally.bytes), Some(7));
    }

    fn file_entry(name: &str, size: u64, attributes: Attributes) -> Entry {
        let mut entry = Entry::new(name, EntryKind::File);
        entry.size = size;
        entry.attributes = attributes;
        entry
    }

    fn item(entry: Entry) -> Item {
        Item {
            path: at(&entry.name.to_string_lossy()),
            entry: Some(entry),
            unreadable: None,
            unlisted: None,
            link: None,
        }
    }

    /// What the comparison says, from the rows as the window makes them.
    fn judged(properties: &Properties) -> Verdict {
        verdict(properties, &hash_lines(properties))
    }

    /// The value of the fact `label`, if the window shows it.
    fn fact_value(properties: &Properties, label: &str) -> Option<String> {
        facts(properties, &TimeZone::UTC)
            .into_iter()
            .find(|fact| fact.label == label)
            .map(|fact| fact.value)
    }

    #[test]
    fn the_window_counts_what_was_chosen_and_hashes_its_files_only() {
        let items = vec![
            item(file_entry(
                "a.txt",
                3,
                Attributes::READONLY | Attributes::ARCHIVE,
            )),
            item(Entry::new("docs", EntryKind::Dir)),
            item(file_entry("b.png", 10, Attributes::ARCHIVE)),
            item(Entry::new("link", EntryKind::Symlink)),
        ];
        let properties = Properties::new(1, at(""), false, items);
        assert_eq!(
            properties.chosen,
            Chosen {
                files: 3,
                folders: 1,
                bytes: 13,
                flagged: [1, 0, 0, 2],
            }
        );
        let rows: Vec<usize> = properties.hashing.rows.iter().map(|row| row.item).collect();
        assert_eq!(rows, [0, 2]);
        assert_eq!(properties.hashing.left_out, 0);
        let many: Vec<Item> = (0..MAX_HASHED_FILES + 5)
            .map(|i| item(file_entry(&format!("f{i}"), 1, Attributes::empty())))
            .collect();
        let properties = Properties::new(1, at(""), false, many);
        assert_eq!(properties.hashing.rows.len(), MAX_HASHED_FILES);
        assert_eq!(properties.hashing.left_out, 5);
    }

    #[test]
    fn one_file_shows_its_facts() {
        let mut entry = file_entry(
            "설계안.pdf",
            1_258_291,
            Attributes::READONLY | Attributes::HIDDEN,
        );
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_761_800_000);
        entry.modified = Some(time);
        entry.accessed = Some(time);
        let properties = Properties::new(1, at(""), false, vec![item(entry)]);
        let facts = facts(&properties, &TimeZone::UTC);
        let shown: Vec<(&str, &str)> = facts
            .iter()
            .map(|fact| (fact.label, fact.value.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("종류", "PDF 문서"),
                ("위치", place(&at("").to_string()).as_str()),
                ("크기", "1.2 MB (1,258,291 바이트)"),
                ("만든 날짜", "알 수 없음"),
                ("수정한 날짜", "2025-10-30 04:53"),
                ("접근한 날짜", "2025-10-30 04:53"),
                ("속성", "읽기 전용, 숨김"),
            ]
        );
    }

    #[test]
    fn a_shortened_place_keeps_the_whole_path_beside_it() {
        let mut deep = at("");
        for _ in 0..8 {
            deep = deep.join("아주 긴 폴더 이름").expect("a name");
        }
        let deep = deep.join("설계안.pdf").expect("a name");
        let properties = Properties::new(
            1,
            at(""),
            false,
            vec![Item {
                path: deep.clone(),
                entry: Some(file_entry("설계안.pdf", 1, Attributes::empty())),
                unreadable: None,
                unlisted: None,
                link: None,
            }],
        );
        let place = facts(&properties, &TimeZone::UTC)
            .into_iter()
            .find(|fact| fact.label == "위치")
            .expect("a place");
        let whole = deep.parent().expect("a parent").to_string();
        assert!(place.value.contains('…'), "{}", place.value);
        assert_eq!(place.full, Some(whole));
        // A short one has nothing beside it.
        let properties = Properties::new(
            1,
            at(""),
            false,
            vec![item(file_entry("a", 1, Attributes::empty()))],
        );
        let place = facts(&properties, &TimeZone::UTC)
            .into_iter()
            .find(|fact| fact.label == "위치")
            .expect("a place");
        assert_eq!(place.full, None);
    }

    #[test]
    fn a_long_place_keeps_its_end_on_one_line() {
        let long = format!("/home/me/{}/사진/2026/여행", "아주 긴 폴더 이름 ".repeat(8));
        let shown = place(&long);
        assert!(shown.chars().count() <= PLACE_CHARS, "{shown}");
        assert!(
            shown.starts_with('/') && shown.ends_with("/사진/2026/여행"),
            "{shown}"
        );
        assert_eq!(place("/home/me"), "/home/me");
    }

    #[test]
    fn a_folder_shows_its_count_as_it_goes() {
        let mut properties = Properties::new(
            1,
            at(""),
            false,
            vec![item(Entry::new("docs", EntryKind::Dir))],
        );
        properties.count = Count {
            generation: 2,
            tally: Tally {
                files: 12_345,
                folders: 678,
                bytes: 1_288_490_188,
                unreadable: 3,
                elsewhere: 0,
            },
            status: CountStatus::Counting,
        };
        let value = |properties: &Properties, label: &str| {
            facts(properties, &TimeZone::UTC)
                .into_iter()
                .find(|fact| fact.label == label)
                .map(|fact| fact.value)
        };
        assert_eq!(
            value(&properties, "크기").as_deref(),
            Some("1.2 GB (1,288,490,188 바이트) 이상")
        );
        assert_eq!(
            value(&properties, "내용").as_deref(),
            Some("파일 12,345개, 폴더 678개")
        );
        assert_eq!(
            count_note(&properties),
            "세는 중…\n읽을 수 없는 폴더 3개는 세지 못했습니다"
        );
        properties.count.status = CountStatus::Done;
        properties.count.tally.unreadable = 0;
        assert_eq!(
            value(&properties, "크기").as_deref(),
            Some("1.2 GB (1,288,490,188 바이트)")
        );
        assert_eq!(count_note(&properties), "");
        properties.count.status = CountStatus::Stopped;
        properties.count.tally.elsewhere = 2;
        assert_eq!(
            count_note(&properties),
            "세다가 멈췄습니다\n다른 드라이브의 폴더 2개는 세지 않았습니다"
        );
    }

    #[test]
    fn several_items_show_their_totals() {
        let items = vec![
            item(file_entry("a.txt", 1_000, Attributes::READONLY)),
            item(file_entry("b.txt", 2_000, Attributes::READONLY)),
            item(Entry::new("docs", EntryKind::Dir)),
        ];
        let mut properties = Properties::new(1, at(""), false, items);
        properties.count.status = CountStatus::Done;
        properties.count.tally = Tally {
            files: 4,
            folders: 1,
            bytes: 7_000,
            ..Tally::default()
        };
        let shown: Vec<(&str, String)> = facts(&properties, &TimeZone::UTC)
            .into_iter()
            .map(|fact| (fact.label, fact.value))
            .collect();
        assert_eq!(
            shown,
            [
                ("종류", "파일 2개, 폴더 1개".to_owned()),
                ("위치", place(&at("").to_string())),
                ("크기", "9.8 KB (10,000 바이트)".to_owned()),
                ("내용", "파일 4개, 폴더 1개".to_owned()),
                ("속성", "읽기 전용 (일부)".to_owned()),
            ]
        );
    }

    #[test]
    fn the_comparison_says_match_mismatch_or_waits() {
        let items = vec![
            item(file_entry("a.txt", 3, Attributes::empty())),
            item(file_entry("b.txt", 3, Attributes::empty())),
        ];
        let mut properties = Properties::new(1, at(""), false, items);
        properties.compare = "  ABC123 \n".to_owned();
        // No algorithm, nothing to say.
        assert_eq!(judged(&properties), Verdict::None);
        properties.hashing.algorithm = Some(Algorithm::Md5);
        properties.hashing.rows[0].status = HashStatus::Done("abc123".to_owned());
        properties.hashing.rows[1].status = HashStatus::Reading { done: 1, total: 3 };
        assert_eq!(
            judged(&properties),
            Verdict::Match(Some("a.txt".to_owned()), 1)
        );
        assert!(hash_lines(&properties)[0].matched);
        properties.compare = "def".to_owned();
        assert_eq!(judged(&properties), Verdict::Waiting);
        properties.hashing.rows[1].status = HashStatus::Done("fed".to_owned());
        assert_eq!(judged(&properties), Verdict::Mismatch);
        properties.compare = " ".to_owned();
        assert_eq!(judged(&properties), Verdict::None);
        assert_eq!(
            hash_lines(&properties)
                .iter()
                .map(|line| line.value.as_str())
                .collect::<Vec<_>>(),
            ["abc123", "fed"]
        );
        // A row 중지 stopped may still be made: no mismatch before it is.
        properties.compare = "abc".to_owned();
        properties.hashing.rows[1].status = HashStatus::Stopped;
        assert_eq!(judged(&properties), Verdict::Waiting);
        assert_eq!(hash_lines(&properties)[1].value, "멈춤");
        assert!(!properties.hashing.under_way());
        assert_eq!(
            hash_note(&properties),
            "계산을 멈췄습니다. 같은 알고리즘을 다시 고르면 남은 파일을 계산합니다"
        );
    }

    #[test]
    fn a_count_that_met_a_folder_it_could_not_list_is_not_whole() {
        let mut properties = Properties::new(
            1,
            at(""),
            false,
            vec![item(Entry::new("docs", EntryKind::Dir))],
        );
        properties.count = Count {
            generation: 2,
            tally: Tally {
                files: 3,
                folders: 2,
                bytes: 2_048,
                unreadable: 1,
                elsewhere: 0,
            },
            status: CountStatus::Done,
        };
        assert_eq!(
            fact_value(&properties, "크기").as_deref(),
            Some("2 KB (2,048 바이트) 이상")
        );
        assert_eq!(
            count_note(&properties),
            "읽을 수 없는 폴더 1개는 세지 못했습니다"
        );
    }

    #[test]
    fn a_folder_that_cannot_be_listed_or_read_says_why_rather_than_look_empty() {
        // A chosen folder the worker cannot list: it tells which.
        let (vfs, _) = vfs_of(tree());
        let mut news = Vec::new();
        let tally = measure(
            &request(
                vec![MeasureItem {
                    path: at("locked"),
                    entry: Some(on("locked", EntryKind::Dir, 0, 1, 3)),
                }],
                true,
            ),
            &vfs,
            &CancelToken::new(),
            Duration::ZERO,
            &mut |item| news.push(item),
        );
        assert_eq!(tally.map(|tally| tally.unreadable), Some(1));
        assert!(
            news.contains(&MeasureNews::Unlisted {
                generation: 5,
                item: 0,
                kind: ErrorKind::PermissionDenied,
            }),
            "{news:?}"
        );
        // The window says so in place of the size, and no contents of 0 items.
        let mut locked = item(Entry::new("locked", EntryKind::Dir));
        locked.unlisted = Some(ErrorKind::PermissionDenied);
        let mut properties = Properties::new(1, at(""), false, vec![locked]);
        properties.count.status = CountStatus::Done;
        properties.count.tally = tally.unwrap_or_default();
        assert_eq!(
            fact_value(&properties, "크기").as_deref(),
            Some("폴더를 읽을 수 없습니다: 권한이 없습니다")
        );
        assert_eq!(fact_value(&properties, "내용"), None);
        assert_eq!(count_note(&properties), "");
        // The folder a pane shows, which the worker could not read at all.
        let mut properties = Properties::new(
            1,
            at(""),
            true,
            vec![Item {
                path: at("gone"),
                entry: None,
                unreadable: Some(ErrorKind::NotFound),
                unlisted: None,
                link: None,
            }],
        );
        properties.count.status = CountStatus::Done;
        assert_eq!(
            fact_value(&properties, "크기").as_deref(),
            Some("찾을 수 없습니다")
        );
        assert_eq!(fact_value(&properties, "내용"), None);
    }
}
