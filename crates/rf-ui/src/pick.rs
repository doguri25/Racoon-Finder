//! The destination picker of a copy or a move (PICK, PRD 5.5), as plain data.
//!
//! [`PickRequest`] is what goes where from: the items, their size, the folder they are in,
//! and whether they are copied or moved. [`quick_targets`] gives the folders the picker
//! offers on its right: the folder of the other pane first, which is chosen when the picker
//! opens (PICK-05's default), then the folders of the open tabs, the recent targets and the
//! favorites, each folder once. [`validate`] tells whether a folder can take the items: an
//! item itself, a folder inside one and, for a move, the folder the items are in cannot be
//! chosen (PICK-06, DND-09), nor, once a worker has looked, a path that is no folder; a
//! read-only location or too little room only warns, since the job checks again and the user
//! may know better (PICK-06: a warning before the confirmation).
//!
//! [`Picker`] is the picker's state while it shows: the request, the offered folders, the
//! chosen one and what a worker found about it ([`TargetFacts`]), the path typed, the filter
//! of its folder tree and the conflict policy. [`AppState`](crate::state::AppState) keeps it
//! in its queue of windows and does the rest: the picker's folder tree, the workers and the
//! job.

use std::collections::HashSet;
use std::ffi::OsString;

use rf_core::{ErrorKind, VPath};
use rf_jobs::{ConflictPolicy, TransferMode, TransferOptions, TransferSpec};

use crate::format::{folder_label, short_path};
use crate::pane::next_generation;
use crate::places::{Place, PlaceKind};
use crate::words;

/// How many recent targets the picker offers and the app keeps.
pub const RECENT_TARGETS: usize = 10;

/// How many characters the path under a quick target shows; the middle of a longer one is
/// left out, so that its end stays in view.
const NOTE_CHARS: usize = 56;

/// The conflict policies the picker offers, in the order of its buttons: 물어보기, 덮어쓰기,
/// 건너뛰기, 이름 바꾸기.
pub const POLICIES: [ConflictPolicy; 4] = [
    ConflictPolicy::Ask,
    ConflictPolicy::Overwrite,
    ConflictPolicy::Skip,
    ConflictPolicy::AutoRename,
];

/// What the picker is asked to find a folder for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRequest {
    pub mode: TransferMode,
    /// The items, all in `from_dir`.
    pub sources: Vec<VPath>,
    /// The bytes of the items that are no folder; what is inside a folder is not counted.
    pub file_bytes: u64,
    /// How many of the items are folders.
    pub folder_count: usize,
    /// How many of the items are links or other items that are neither a file nor a folder:
    /// they have no size to add.
    pub other_count: usize,
    /// The folder the items are in.
    pub from_dir: VPath,
}

impl PickRequest {
    /// How many of the items are files: neither a folder nor a link or the like.
    pub fn file_count(&self) -> usize {
        self.sources
            .len()
            .saturating_sub(self.folder_count)
            .saturating_sub(self.other_count)
    }
}

/// Whether a folder can take the items of a [`PickRequest`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validity {
    Ok,
    /// A move into the folder the items are in already: it would do nothing (DND-09).
    SameFolder,
    /// One of the items, or a folder inside one.
    IntoSource,
    /// The path is no folder, or nothing is there (any more).
    NotAFolder,
    /// The location is read-only: a warning, the job may still fail.
    NotWritable,
    /// The files need more room than is free there: a warning. With folders among the items
    /// `need` counts the files alone, so the job may need more.
    NotEnoughSpace {
        need: u64,
        available: u64,
    },
}

impl Validity {
    /// Whether the folder can be chosen: with no problem, or with a warning.
    pub fn can_choose(self) -> bool {
        matches!(
            self,
            Validity::Ok | Validity::NotWritable | Validity::NotEnoughSpace { .. }
        )
    }
}

/// What a worker found at a target ([`Loader::inspect_target`](crate::loader::Loader::inspect_target)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetFacts {
    /// `Ok` when the path is a folder (or a link to one); else why not: `NotADirectory` for
    /// an item that is no folder, the error's kind when it could not be looked at.
    pub folder: Result<(), ErrorKind>,
    /// Whether the location takes changes (`Capabilities::WRITE`).
    pub writable: bool,
    /// The bytes the user can still write there, when the location tells.
    pub available: Option<u64>,
    /// Whether the folder is on the volume of the folder the items are in: a move there only
    /// renames, and needs no room.
    pub same_volume: bool,
}

/// One folder the picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: VPath,
    /// The name it shows: the folder's own, or a favorite's.
    pub label: String,
    /// The path under the name, its middle left out when it is long.
    pub note: String,
    /// Whether it can be chosen, as far as is known without looking at it.
    pub validity: Validity,
}

/// The folders under one head of the picker's quick targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetGroup {
    pub title: String,
    pub items: Vec<Target>,
}

/// The picker's line under its title: `항목 3개 · 1.2 GB`, or with folders, whose size is
/// not known before the job looks inside them, `항목 3개 · 파일 1.2 MB와 폴더 1개`. Links
/// count as items, with no size.
pub fn summary(req: &PickRequest) -> String {
    words::pick_summary(
        req.sources.len(),
        req.file_count(),
        req.file_bytes,
        req.folder_count,
    )
}

/// Whether `target` can take the items of `req`, with what a worker found there (`facts`)
/// once it has. Without facts only what the paths tell is checked: the items themselves and
/// the folders inside them, and the folder of a move.
///
/// Paths are compared as they are spelled, name by name: a path typed in another letter case
/// on a volume that ignores case is not caught here, and the job refuses it instead.
pub fn validate(req: &PickRequest, target: &VPath, facts: Option<&TargetFacts>) -> Validity {
    if req.sources.iter().any(|source| target.starts_with(source)) {
        return Validity::IntoSource;
    }
    if req.mode == TransferMode::Move && *target == req.from_dir {
        return Validity::SameFolder;
    }
    let Some(facts) = facts else {
        return Validity::Ok;
    };
    if facts.folder.is_err() {
        return Validity::NotAFolder;
    }
    if !facts.writable {
        return Validity::NotWritable;
    }
    // A move on one volume only renames.
    let needs_room = req.mode == TransferMode::Copy || !facts.same_volume;
    match facts.available {
        Some(available) if needs_room && req.file_bytes > available => Validity::NotEnoughSpace {
            need: req.file_bytes,
            available,
        },
        _ => Validity::Ok,
    }
}

/// The folders the picker offers for `req`, under their heads: the folder of the other pane
/// (`opposite`), the folders of the open tabs, the `recent` targets (the first
/// [`RECENT_TARGETS`]), and the favorites among `places` (the drives are in the tree). A
/// folder shows once, under the first head that has it, and a head without folders is left
/// out.
pub fn quick_targets(
    req: &PickRequest,
    opposite: &VPath,
    open_tabs: &[VPath],
    recent: &[VPath],
    places: &[Place],
) -> Vec<TargetGroup> {
    let named = |path: &VPath| (folder_label(path), path.clone());
    let heads: [(&str, Vec<(String, VPath)>); 4] = [
        (words::pick_opposite(), vec![named(opposite)]),
        (words::pick_tabs(), open_tabs.iter().map(named).collect()),
        (
            words::pick_recent(),
            recent.iter().take(RECENT_TARGETS).map(named).collect(),
        ),
        (
            words::pick_favorites(),
            places
                .iter()
                .filter(|place| matches!(place.kind, PlaceKind::Favorite(_)))
                .map(|place| (place.label.clone(), place.path.clone()))
                .collect(),
        ),
    ];
    let mut seen: HashSet<VPath> = HashSet::new();
    heads
        .into_iter()
        .filter_map(|(title, folders)| {
            let items: Vec<Target> = folders
                .into_iter()
                .filter(|(_, path)| seen.insert(path.clone()))
                .map(|(label, path)| Target {
                    note: short_path(&path.to_string(), NOTE_CHARS),
                    validity: validate(req, &path, None),
                    label,
                    path,
                })
                .collect();
            (!items.is_empty()).then(|| TargetGroup {
                title: title.to_owned(),
                items,
            })
        })
        .collect()
}

/// The picker while it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub request: PickRequest,
    /// The pane the items are in.
    pub origin: usize,
    pub groups: Vec<TargetGroup>,
    /// The folder the items go to, once one is chosen.
    pub target: Option<VPath>,
    /// What a worker found at `target`; `None` until it has told.
    pub facts: Option<TargetFacts>,
    /// The generation of the last look a worker was asked to take at the target. The result
    /// of an earlier one is late, and dropped.
    pub looking: u64,
    /// A path typed into the path field, being looked at: it becomes the target once it is
    /// known to be a folder. Nothing is confirmed meanwhile.
    pub typed: Option<VPath>,
    /// The generation of the look at `typed`, apart from the target's: a refused path leaves
    /// the target's look to come.
    pub typing: u64,
    /// Why the path typed cannot be the target; empty when nothing is wrong with it.
    pub path_error: String,
    /// The path field holds text typed into it that is not the target's path: nothing is
    /// confirmed until it is entered and found to be a folder, or another target is chosen.
    pub path_edited: bool,
    /// What the filter of the folder tree holds.
    pub filter: String,
    /// The filter has changed since a target was last chosen: Enter takes the first folder it
    /// finds that can be chosen, rather than confirming.
    pub filter_fresh: bool,
    pub policy: ConflictPolicy,
    /// The folder 새 폴더 asked for, by its folder and its name: it becomes the target once
    /// its job has made it.
    pub making: Option<(VPath, OsString)>,
}

/// What [`Picker::looked`] did with a worker's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Looked {
    /// It was late: the picker has looked elsewhere since.
    Stale,
    /// It told about the target.
    Facts,
    /// The typed path is a folder, and the target now.
    Chosen,
    /// The typed path cannot be the target; [`Picker::path_error`] says why.
    Refused,
}

impl Picker {
    /// The picker of `request` for the items of pane `origin`, offering `groups`, with the
    /// first folder that can be chosen as the target, conflicts asked about, and nothing
    /// looked at yet.
    pub fn new(request: PickRequest, origin: usize, groups: Vec<TargetGroup>) -> Picker {
        let target = groups
            .iter()
            .flat_map(|group| &group.items)
            .find(|item| item.validity.can_choose())
            .map(|item| item.path.clone());
        Picker {
            request,
            origin,
            groups,
            target,
            facts: None,
            looking: 0,
            typed: None,
            typing: 0,
            path_error: String::new(),
            path_edited: false,
            filter: String::new(),
            filter_fresh: false,
            policy: ConflictPolicy::Ask,
            making: None,
        }
    }

    /// The quick targets in the order they show, every head's in turn.
    pub fn targets(&self) -> impl Iterator<Item = &Target> {
        self.groups.iter().flat_map(|group| &group.items)
    }

    /// Whether the target can take the items, with what is known about it; `None` without a
    /// target.
    pub fn validity(&self) -> Option<Validity> {
        let target = self.target.as_ref()?;
        Some(validate(&self.request, target, self.facts.as_ref()))
    }

    /// Whether the confirm button works: a target that can be chosen, and the path field
    /// showing it (no path typed and not entered, or entered and still being looked at).
    pub fn can_confirm(&self) -> bool {
        !self.path_edited
            && self.typed.is_none()
            && self.validity().is_some_and(Validity::can_choose)
    }

    /// The quick target `delta` steps from the target among those that can be chosen (an
    /// arrow key): the first one (or with a negative `delta` the last one) when the target is
    /// not among them. `None` when none can be chosen.
    pub fn step(&self, delta: isize) -> Option<VPath> {
        let choosable: Vec<&Target> = self
            .targets()
            .filter(|item| item.validity.can_choose())
            .collect();
        let last = choosable.len().checked_sub(1)?;
        let current = self
            .target
            .as_ref()
            .and_then(|target| choosable.iter().position(|item| item.path == *target));
        let next = match current {
            Some(at) => at.saturating_add_signed(delta).min(last),
            None if delta < 0 => last,
            None => 0,
        };
        Some(choosable[next].path.clone())
    }

    /// Copies or moves from now on, as `mode` says: the quick targets are checked again.
    pub fn set_mode(&mut self, mode: TransferMode) {
        self.request.mode = mode;
        for group in &mut self.groups {
            for item in &mut group.items {
                item.validity = validate(&self.request, &item.path, None);
            }
        }
    }

    /// Makes `path` the target, with nothing known about it yet, and gives the generation of
    /// the look a worker is to take at it. The path field shows it from now on, whatever was
    /// typed there, and the filter's first folder is no longer taken by Enter.
    pub fn choose(&mut self, path: VPath) -> u64 {
        self.target = Some(path);
        self.facts = None;
        self.typed = None;
        self.path_error.clear();
        self.path_edited = false;
        self.filter_fresh = false;
        self.looking = next_generation();
        self.looking
    }

    /// The path typed into the path field: a worker looks at it before it is chosen. Gives
    /// the generation of that look; the target's own look goes on.
    pub fn type_path(&mut self, path: VPath) -> u64 {
        self.typed = Some(path);
        self.path_error.clear();
        self.typing = next_generation();
        self.typing
    }

    /// The path field now holds `text`, typed but not entered: unless it is the target's path,
    /// nothing is confirmed until it is. A path entered before and still being looked at is
    /// forgotten, and so is why the last one was refused.
    pub fn edit_path(&mut self, text: &str) {
        self.path_edited = self
            .target
            .as_ref()
            .is_none_or(|target| target.to_string() != text);
        self.typed = None;
        self.path_error.clear();
    }

    /// The filter of the tree now holds `text`.
    pub fn set_filter(&mut self, text: &str) {
        if self.filter != text {
            text.clone_into(&mut self.filter);
            self.filter_fresh = !text.trim().is_empty();
        }
    }

    /// Takes what a worker found at `path` for the look `generation`: at the target, or at a
    /// typed path, which becomes the target if it is a folder.
    pub fn looked(&mut self, generation: u64, path: &VPath, facts: TargetFacts) -> Looked {
        if generation == self.typing && self.typed.as_ref() == Some(path) {
            self.typed = None;
            return match facts.folder {
                Ok(()) => {
                    self.target = Some(path.clone());
                    self.facts = Some(facts);
                    self.path_edited = false;
                    self.filter_fresh = false;
                    Looked::Chosen
                }
                Err(kind) => {
                    self.path_error = words::pick_path_error(kind);
                    Looked::Refused
                }
            };
        }
        if generation != self.looking || self.target.as_ref() != Some(path) {
            return Looked::Stale;
        }
        self.facts = Some(facts);
        Looked::Facts
    }

    /// The transfer the confirm button submits; `None` without a target.
    pub fn spec(&self) -> Option<rf_core::Result<TransferSpec>> {
        let target = self.target.as_ref()?;
        Some(TransferSpec::into_dir(
            self.request.mode,
            &self.request.sources,
            target,
            TransferOptions {
                conflict: self.policy,
                ..TransferOptions::default()
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    /// `base` with `names` below it.
    fn under(base: &VPath, names: &[&str]) -> VPath {
        names.iter().fold(base.clone(), |path, name| {
            path.join(name).expect("a plain name")
        })
    }

    /// A request for the folder `docs` and the file `x.txt` of the folder `a`.
    fn request(mode: TransferMode) -> PickRequest {
        let a = folder("a");
        PickRequest {
            mode,
            sources: vec![under(&a, &["docs"]), under(&a, &["x.txt"])],
            file_bytes: 2 * MB,
            folder_count: 1,
            other_count: 0,
            from_dir: a,
        }
    }

    fn facts(available: Option<u64>, same_volume: bool) -> TargetFacts {
        TargetFacts {
            folder: Ok(()),
            writable: true,
            available,
            same_volume,
        }
    }

    fn favorite(label: &str, path: &VPath) -> Place {
        Place {
            label: label.to_owned(),
            path: path.clone(),
            kind: PlaceKind::Favorite(rf_platform::KnownFolder::Home),
            space: None,
        }
    }

    /// The groups as `(title, [label, …])`, a target that cannot be chosen marked with `-`.
    fn shown(groups: &[TargetGroup]) -> Vec<(String, Vec<String>)> {
        groups
            .iter()
            .map(|group| {
                let labels = group
                    .items
                    .iter()
                    .map(|item| {
                        let mark = if item.validity.can_choose() { "" } else { "-" };
                        format!("{mark}{}", item.label)
                    })
                    .collect();
                (group.title.clone(), labels)
            })
            .collect()
    }

    fn group(title: &str, labels: &[&str]) -> (String, Vec<String>) {
        (
            title.to_owned(),
            labels.iter().map(|&label| label.to_owned()).collect(),
        )
    }

    #[test]
    fn pick_rejects_source_and_descendants() {
        let a = folder("a");
        let docs = under(&a, &["docs"]);
        for mode in [TransferMode::Move, TransferMode::Copy] {
            let req = request(mode);
            // An item itself, and every folder inside it, however deep.
            for target in [
                docs.clone(),
                under(&docs, &["sub"]),
                under(&docs, &["sub", "deeper"]),
            ] {
                assert_eq!(validate(&req, &target, None), Validity::IntoSource);
                // What a worker found changes nothing.
                assert_eq!(
                    validate(&req, &target, Some(&facts(Some(GB), true))),
                    Validity::IntoSource
                );
            }
            // A folder whose name only starts the same is another folder.
            assert_eq!(validate(&req, &under(&a, &["docs2"]), None), Validity::Ok);
            assert!(!Validity::IntoSource.can_choose());
        }

        // A tab inside a folder that moves is offered, but cannot be chosen, and is not the
        // default.
        let req = request(TransferMode::Move);
        let inside = under(&docs, &["sub"]);
        let groups = quick_targets(&req, &inside, &[folder("b")], &[], &[]);
        assert_eq!(
            shown(&groups),
            [group("반대편 패널", &["-sub"]), group("열린 탭", &["b"])]
        );
        assert_eq!(groups[0].items[0].validity, Validity::IntoSource);
        let picker = Picker::new(req, 0, groups);
        assert_eq!(picker.target, Some(folder("b")));
    }

    #[test]
    fn move_to_same_folder_is_rejected_copy_is_allowed() {
        let a = folder("a");
        let moving = request(TransferMode::Move);
        let copying = request(TransferMode::Copy);
        assert_eq!(validate(&moving, &a, None), Validity::SameFolder);
        assert!(!Validity::SameFolder.can_choose());
        // A copy into the folder the items are in makes copies beside them.
        assert_eq!(validate(&copying, &a, None), Validity::Ok);

        // A path that is no folder cannot be chosen once a worker has looked.
        let b = folder("b");
        let gone = TargetFacts {
            folder: Err(ErrorKind::NotFound),
            ..facts(None, false)
        };
        assert_eq!(validate(&copying, &b, Some(&gone)), Validity::NotAFolder);
        assert!(!Validity::NotAFolder.can_choose());

        // A read-only location and too little room only warn.
        let read_only = TargetFacts {
            writable: false,
            ..facts(Some(GB), false)
        };
        assert_eq!(
            validate(&copying, &b, Some(&read_only)),
            Validity::NotWritable
        );
        let full = facts(Some(MB), false);
        let too_little = Validity::NotEnoughSpace {
            need: 2 * MB,
            available: MB,
        };
        assert_eq!(validate(&copying, &b, Some(&full)), too_little);
        assert!(Validity::NotWritable.can_choose() && too_little.can_choose());
        // A move to another volume needs the room too; on the same volume it only renames.
        assert_eq!(validate(&moving, &b, Some(&full)), too_little);
        assert_eq!(
            validate(&moving, &b, Some(&facts(Some(MB), true))),
            Validity::Ok
        );
        // Room that is not known is no warning.
        assert_eq!(
            validate(&copying, &b, Some(&facts(None, false))),
            Validity::Ok
        );

        // The mode can change while the picker shows: the folder of the items becomes a
        // target of a copy, and stops being one of a move.
        let groups = quick_targets(&moving, &b, std::slice::from_ref(&a), &[], &[]);
        let mut picker = Picker::new(moving, 0, groups);
        assert_eq!(picker.groups[1].items[0].validity, Validity::SameFolder);
        picker.set_mode(TransferMode::Copy);
        assert_eq!(picker.groups[1].items[0].validity, Validity::Ok);
        picker.choose(a.clone());
        assert!(picker.can_confirm());
        picker.set_mode(TransferMode::Move);
        assert_eq!(picker.validity(), Some(Validity::SameFolder));
        assert!(!picker.can_confirm());
    }

    #[test]
    fn opposite_pane_comes_first_and_is_default() {
        let req = request(TransferMode::Copy);
        let (b, c, home) = (folder("b"), folder("c"), folder("home"));
        let docs = under(&home, &["문서"]);
        let drive = Place {
            kind: PlaceKind::Drive,
            ..favorite("시스템 디스크", &folder("drive"))
        };
        let groups = quick_targets(
            &req,
            &b,
            &[folder("a"), c.clone()],
            &[under(&c, &["백업"])],
            &[favorite("홈", &home), favorite("문서", &docs), drive],
        );
        // The drives are in the tree, not among the quick targets.
        assert_eq!(
            shown(&groups),
            [
                group("반대편 패널", &["b"]),
                group("열린 탭", &["a", "c"]),
                group("최근 위치", &["백업"]),
                group("즐겨찾기", &["홈", "문서"]),
            ]
        );
        // Each shows its path under its name.
        assert_eq!(groups[3].items[1].note, docs.to_string());
        let picker = Picker::new(req, 1, groups);
        assert_eq!(picker.target, Some(b));
        assert_eq!(picker.origin, 1);
        assert_eq!(picker.policy, ConflictPolicy::Ask);
        assert_eq!(picker.validity(), Some(Validity::Ok));
        assert!(picker.can_confirm());

        // A move whose other pane shows the items' own folder starts at the next folder that
        // can take them.
        let a = folder("a");
        let req = request(TransferMode::Move);
        let groups = quick_targets(&req, &a, std::slice::from_ref(&c), &[], &[]);
        assert_eq!(
            shown(&groups),
            [group("반대편 패널", &["-a"]), group("열린 탭", &["c"])]
        );
        assert_eq!(Picker::new(req, 0, groups).target, Some(c));

        // Nothing to choose: no target, and nothing to confirm.
        let req = request(TransferMode::Move);
        let groups = quick_targets(&req, &a, &[], &[], &[]);
        let picker = Picker::new(req, 0, groups);
        assert_eq!(picker.target, None);
        assert_eq!(picker.validity(), None);
        assert!(!picker.can_confirm());
        assert!(picker.spec().is_none());
    }

    #[test]
    fn duplicates_are_removed() {
        let req = request(TransferMode::Copy);
        let (b, c, d) = (folder("b"), folder("c"), folder("d"));
        let recent: Vec<VPath> = (0..14).map(|i| folder(&format!("r{i}"))).collect();
        let mut with_repeats = vec![c.clone(), d.clone(), b.clone()];
        with_repeats.extend(recent.iter().cloned());
        let groups = quick_targets(
            &req,
            &b,
            &[b.clone(), c.clone(), c.clone()],
            &with_repeats,
            &[favorite("D", &d), favorite("E", &folder("e"))],
        );
        // Each folder once, under the first head that has it; the ten most recent targets
        // only, a repeated one counting among them.
        let mut recent_labels = vec!["d".to_owned()];
        recent_labels.extend((0..7).map(|i| format!("r{i}")));
        let recent_labels: Vec<&str> = recent_labels.iter().map(String::as_str).collect();
        assert_eq!(
            shown(&groups),
            [
                group("반대편 패널", &["b"]),
                group("열린 탭", &["c"]),
                group("최근 위치", &recent_labels),
                group("즐겨찾기", &["E"]),
            ]
        );
        let paths: Vec<&VPath> = groups
            .iter()
            .flat_map(|group| &group.items)
            .map(|item| &item.path)
            .collect();
        let unique: HashSet<&VPath> = paths.iter().copied().collect();
        assert_eq!(unique.len(), paths.len());
    }

    #[test]
    fn summary_counts_files_and_folders() {
        let a = folder("a");
        let files = PickRequest {
            mode: TransferMode::Move,
            sources: vec![
                under(&a, &["1.txt"]),
                under(&a, &["2.txt"]),
                under(&a, &["3.txt"]),
            ],
            file_bytes: 1_288_490_189,
            folder_count: 0,
            other_count: 0,
            from_dir: a.clone(),
        };
        assert_eq!(summary(&files), "항목 3개 · 1.2 GB");
        let mixed = PickRequest {
            file_bytes: 1_258_291,
            folder_count: 1,
            ..files.clone()
        };
        assert_eq!(mixed.file_count(), 2);
        assert_eq!(summary(&mixed), "항목 3개 · 파일 1.2 MB와 폴더 1개");
        let folders = PickRequest {
            sources: vec![under(&a, &["x"]), under(&a, &["y"])],
            file_bytes: 0,
            folder_count: 2,
            ..files.clone()
        };
        assert_eq!(summary(&folders), "항목 2개 · 폴더 2개");
        // A link moves as a link: it is an item, with no size of its own.
        let link = PickRequest {
            sources: vec![under(&a, &["latest"])],
            file_bytes: 0,
            folder_count: 0,
            other_count: 1,
            ..folders.clone()
        };
        assert_eq!(link.file_count(), 0);
        assert_eq!(summary(&link), "항목 1개");
        let with_link = PickRequest {
            sources: vec![under(&a, &["latest"]), under(&a, &["x"])],
            file_bytes: 0,
            folder_count: 1,
            other_count: 1,
            ..folders
        };
        assert_eq!(summary(&with_link), "항목 2개 · 폴더 1개");
    }

    #[test]
    fn arrows_move_among_the_targets_that_can_be_chosen() {
        let a = folder("a");
        let docs = under(&a, &["docs"]);
        let (b, c) = (folder("b"), folder("c"));
        let req = request(TransferMode::Move);
        let groups = quick_targets(&req, &b, &[a, under(&docs, &["sub"]), c.clone()], &[], &[]);
        let mut picker = Picker::new(req, 0, groups);
        assert_eq!(picker.target, Some(b.clone()));
        // The items' folder and the folder inside an item are passed over.
        assert_eq!(picker.step(1), Some(c.clone()));
        picker.choose(c.clone());
        assert_eq!(picker.step(1), Some(c.clone()));
        assert_eq!(picker.step(-1), Some(b.clone()));
        assert_eq!(picker.step(-5), Some(b.clone()));
        // From a target chosen elsewhere (the tree, a typed path), down starts at the first
        // and up at the last.
        picker.choose(folder("elsewhere"));
        assert_eq!(picker.step(1), Some(b));
        assert_eq!(picker.step(-1), Some(c));
    }

    #[test]
    fn late_looks_are_dropped_and_typed_paths_wait_for_theirs() {
        let req = request(TransferMode::Copy);
        let (b, c) = (folder("b"), folder("c"));
        let groups = quick_targets(&req, &b, &[], &[], &[]);
        let mut picker = Picker::new(req, 0, groups);
        let first = picker.choose(b.clone());
        let second = picker.choose(c.clone());
        assert_ne!(first, second);
        // The look at `b` comes after `c` was chosen: it says nothing about `c`.
        assert_eq!(
            picker.looked(first, &b, facts(Some(MB), false)),
            Looked::Stale
        );
        assert_eq!(picker.facts, None);
        assert_eq!(
            picker.looked(second, &c, facts(Some(MB), false)),
            Looked::Facts
        );
        assert_eq!(
            picker.validity(),
            Some(Validity::NotEnoughSpace {
                need: 2 * MB,
                available: MB
            })
        );
        assert!(picker.can_confirm());

        // A typed path that is no folder stays out, and says why.
        let file = folder("notes.txt");
        let look = picker.type_path(file.clone());
        let not_a_folder = TargetFacts {
            folder: Err(ErrorKind::NotADirectory),
            ..facts(None, false)
        };
        assert_eq!(picker.looked(look, &file, not_a_folder), Looked::Refused);
        assert_eq!(picker.target, Some(c.clone()));
        assert_eq!(picker.path_error, "폴더가 아닙니다");
        // One that is a folder becomes the target, with what was found there.
        let d = folder("d");
        let look = picker.type_path(d.clone());
        assert_eq!(picker.path_error, "");
        assert_eq!(
            picker.looked(look, &d, facts(Some(GB), false)),
            Looked::Chosen
        );
        assert_eq!(picker.target, Some(d));
        assert_eq!(picker.validity(), Some(Validity::Ok));
        // A typed path whose look is overtaken by a click is forgotten.
        let look = picker.type_path(folder("e"));
        picker.choose(b.clone());
        assert_eq!(
            picker.looked(look, &folder("e"), facts(None, false)),
            Looked::Stale
        );
        assert_eq!(picker.target, Some(b));
    }

    #[test]
    fn nothing_is_confirmed_while_the_path_field_says_otherwise() {
        let req = request(TransferMode::Move);
        let (b, c) = (folder("b"), folder("c"));
        let groups = quick_targets(&req, &b, &[], &[], &[]);
        let mut picker = Picker::new(req, 0, groups);
        assert!(picker.can_confirm());
        // Text typed into the field and not entered: the button would not go where the field
        // says.
        picker.edit_path(&c.to_string());
        assert!(picker.path_edited);
        assert!(!picker.can_confirm());
        // Typed back to the target's own path, it says the target again.
        picker.edit_path(&b.to_string());
        assert!(!picker.path_edited && picker.can_confirm());
        // Entered: nothing is confirmed until the worker has looked.
        picker.edit_path(&c.to_string());
        let look = picker.type_path(c.clone());
        assert!(!picker.can_confirm());
        assert_eq!(picker.looked(look, &c, facts(None, false)), Looked::Chosen);
        assert!(!picker.path_edited && picker.can_confirm());
        assert_eq!(picker.target, Some(c.clone()));

        // A path refused keeps the field's text, and the button off, until another target is
        // chosen.
        let file = folder("notes.txt");
        picker.edit_path(&file.to_string());
        let look = picker.type_path(file.clone());
        let not_a_folder = TargetFacts {
            folder: Err(ErrorKind::NotADirectory),
            ..facts(None, false)
        };
        assert_eq!(picker.looked(look, &file, not_a_folder), Looked::Refused);
        assert!(picker.path_edited && !picker.can_confirm());
        picker.choose(b);
        assert!(!picker.path_edited && picker.can_confirm());
        // Editing again forgets a look on its way: its result cannot change the target.
        picker.edit_path(&c.to_string());
        let look = picker.type_path(c.clone());
        picker.edit_path("/somewhere/else");
        assert_eq!(picker.looked(look, &c, facts(None, false)), Looked::Stale);
        assert_eq!(picker.target, Some(folder("b")));
        assert!(!picker.can_confirm());
    }

    #[test]
    fn a_typed_look_leaves_the_targets_look_alone() {
        let req = request(TransferMode::Copy);
        let b = folder("b");
        let groups = quick_targets(&req, &b, &[], &[], &[]);
        let mut picker = Picker::new(req, 0, groups);
        let target_look = picker.choose(b.clone());
        // A path is typed and refused while the target's look is on its way: the target's
        // look still tells about the target.
        let file = folder("notes.txt");
        picker.edit_path(&file.to_string());
        let typed_look = picker.type_path(file.clone());
        let not_a_folder = TargetFacts {
            folder: Err(ErrorKind::NotADirectory),
            ..facts(None, false)
        };
        assert_eq!(
            picker.looked(typed_look, &file, not_a_folder),
            Looked::Refused
        );
        assert_eq!(
            picker.looked(target_look, &b, facts(Some(MB), false)),
            Looked::Facts
        );
        assert_eq!(
            picker.validity(),
            Some(Validity::NotEnoughSpace {
                need: 2 * MB,
                available: MB
            })
        );
    }

    #[test]
    fn the_spec_carries_the_chosen_policy() {
        let req = request(TransferMode::Move);
        let b = folder("b");
        let groups = quick_targets(&req, &b, &[], &[], &[]);
        let mut picker = Picker::new(req, 0, groups);
        picker.policy = POLICIES[3];
        let spec = picker.spec().expect("a target").expect("a transfer");
        assert_eq!(spec.mode, TransferMode::Move);
        assert_eq!(spec.options.conflict, ConflictPolicy::AutoRename);
        assert_eq!(spec.items.len(), 2);
        assert_eq!(spec.items[0].target, under(&b, &["docs"]));
    }
}
