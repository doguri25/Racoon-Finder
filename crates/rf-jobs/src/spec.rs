//! What a job is asked to do: the data the UI hands to the engine, before any file is touched.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::SystemTime;

use rf_core::{Error, Result, VPath};
use rf_platform::{LinkKind, TrashItem};
use serde::{Deserialize, Serialize};

use crate::undo::{CreatedItem, UndoRecord};

/// Whether a transfer keeps the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransferMode {
    Copy,
    Move,
}

/// What to do when the target name already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConflictPolicy {
    /// Stop on the first conflict and ask.
    #[default]
    Ask,
    Overwrite,
    Skip,
    /// Overwrite only when the source is newer than the target (with a tolerance for
    /// coarse file system clocks).
    OverwriteIfNewer,
    OverwriteIfSizeDiffers,
    /// Keep both: the new item gets a numbered name.
    AutoRename,
}

/// What to do when one item fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorPolicy {
    /// Report the failure and carry on with the other items.
    #[default]
    Continue,
    /// Stop at the failure and ask.
    Ask,
    Abort,
}

/// How a copy treats symbolic links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymlinkMode {
    /// Recreate the link itself.
    #[default]
    CopyLink,
    /// Copy what the link points at.
    Follow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeleteMethod {
    Trash,
    Permanent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CreateKind {
    Dir,
    File,
}

/// The policies a transfer runs under. The default asks about conflicts, carries on after
/// errors and copies links as links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransferOptions {
    pub conflict: ConflictPolicy,
    pub errors: ErrorPolicy,
    pub symlinks: SymlinkMode,
}

/// One source and the final path it ends up at.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransferItem {
    pub source: VPath,
    /// The final path, name included, so a retry or an undo can give each item its own
    /// target.
    pub target: VPath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSpec {
    pub mode: TransferMode,
    pub items: Vec<TransferItem>,
    pub options: TransferOptions,
}

impl TransferSpec {
    /// A transfer of `sources` into the folder `dest_dir`, each keeping its own name.
    ///
    /// Fails with `Unsupported` for a source without a name (a volume root), and with the
    /// join error when a name cannot be appended to `dest_dir` (a name that is not UTF-8, from
    /// a Unix disk, into a remote folder).
    pub fn into_dir(
        mode: TransferMode,
        sources: &[VPath],
        dest_dir: &VPath,
        options: TransferOptions,
    ) -> Result<Self> {
        let mut items = Vec::with_capacity(sources.len());
        for source in sources {
            let name = source.file_name().ok_or_else(|| {
                Error::Unsupported(format!("cannot transfer a volume root: {source}"))
            })?;
            items.push(TransferItem {
                source: source.clone(),
                target: dest_dir.join(name)?,
            });
        }
        Ok(Self {
            mode,
            items,
            options,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteSpec {
    pub paths: Vec<VPath>,
    pub method: DeleteMethod,
    pub errors: ErrorPolicy,
}

/// One copy a [`DeleteCopiesSpec`] deletes: the item as its copy job left it (an item of the
/// job's `UndoRecord::Copied`), and the item it was copied from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyOf {
    pub made: CreatedItem,
    pub original: VPath,
}

/// A delete for good of copies that one copy job made: how the user has the copies that the
/// undo of that copy could not send to the trash deleted (OPS-12), since an undo never deletes
/// for good itself. Right before it deletes a copy, the job looks at it and at its original
/// (`copies::check_copy`): a copy goes only while it is as its copy job left it and its
/// original still holds everything it holds. Any other copy stays, and fails as `Stage::Delete`
/// with the reason `CopyKept`; one that is gone is skipped. While the copy is deleted, each
/// item is held to that look again as its step comes: the copy's items by their times, and,
/// once the job has been paused since the look, each item's original by comparing them anew.
/// An item found changed stays with the folders above it. It runs under
/// `ErrorPolicy::Continue` and keeps no undo record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteCopiesSpec {
    pub copies: Vec<CopyOf>,
    /// When the copy job ended (the `at` of its `UndoRecord::Copied`): a copied folder is as
    /// that job left it only while nothing in it is newer. Without it, no copied folder goes.
    pub at: Option<SystemTime>,
    /// Whether a run of this delete may have gone before and deleted part of the copies: a retry
    /// of one (`JobReport::retry_spec`), or a recovered one that a crash cut off after it had
    /// started (`JobEngine::resubmit_recovered`). A copied folder it finds changed is then said
    /// to be partly deleted already (the reason `CopyPartlyDeleted`) rather than changed by
    /// someone else; it stays either way. `false` in a spec written before this was kept.
    #[serde(default)]
    pub resumed: bool,
}

/// A request to the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum JobSpec {
    Transfer(TransferSpec),
    Delete(DeleteSpec),
    /// Deletes for good the copies of a copy job that are still as it left them (see
    /// [`DeleteCopiesSpec`]). Its kind is [`JobKind::Delete`].
    DeleteCopies(DeleteCopiesSpec),
    Rename {
        from: VPath,
        to: VPath,
    },
    Create {
        parent: VPath,
        name: OsString,
        kind: CreateKind,
    },
    /// Makes links (OPS-08): for each `(target, at, kind)` a link of `kind` to `target`, called
    /// as `at` says, and under the first free numbered name when that is taken (`a - 바로 가기
    /// (2).lnk`). The targets are never touched, and a target that is not there still gets its
    /// link, with a warning. The report's undo record is [`UndoRecord::LinksMade`].
    MakeLink {
        links: Vec<(VPath, VPath, LinkKind)>,
    },
    /// Reverses an earlier job as its record says (OPS-12). The report's undo record reverses
    /// this job in turn, which is how a redo is done.
    Undo(UndoRecord),
    /// Puts items back from the trash (NAV-16): each `(item, to)` from its place in the trash to
    /// `to`, its original place or one the user chose, through the provider of `shell://trash`.
    /// Nothing is ever replaced: a taken `to` is a conflict, which the job's resolver answers
    /// (skip, a numbered name, or cancel; an overwrite is refused with `RestoreNeverReplaces`).
    /// Its kind is [`JobKind::Restore`]. The report's undo record sends what came back to the
    /// trash again ([`UndoRecord::Restored`]).
    RestoreFromTrash {
        #[serde(with = "restore_targets")]
        items: Vec<(TrashItem, PathBuf)>,
    },
    /// Deletes items of the trash for good, each with the trash's record of it. Its kind is
    /// [`JobKind::Delete`], and it leaves no undo record: it cannot be undone, which the window
    /// says in the question it asks first.
    ///
    /// Emptying the trash is this job too, of the items the question before it counted: there is
    /// no job that deletes whatever the trash holds when it runs, which would take what came in
    /// after the question.
    DeleteFromTrash {
        items: Vec<TrashItem>,
    },
}

/// The targets of a [`JobSpec::RestoreFromTrash`], kept as the OS's own strings so that no name
/// is lost (a path that is not UTF-8 cannot be stored as text).
mod restore_targets {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use rf_platform::TrashItem;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[allow(
        clippy::ptr_arg,
        reason = "the signature serde's `with` attribute calls"
    )]
    pub fn serialize<S: Serializer>(
        items: &Vec<(TrashItem, PathBuf)>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let items: Vec<(&TrashItem, &std::ffi::OsStr)> = items
            .iter()
            .map(|(item, to)| (item, to.as_os_str()))
            .collect();
        items.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<(TrashItem, PathBuf)>, D::Error> {
        let items = Vec::<(TrashItem, OsString)>::deserialize(deserializer)?;
        Ok(items
            .into_iter()
            .map(|(item, to)| (item, PathBuf::from(to)))
            .collect())
    }
}

/// The sort of job, for lanes, progress text and undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JobKind {
    Copy,
    Move,
    Delete,
    Rename,
    Create,
    /// Making links (`JobSpec::MakeLink`).
    Link,
    Undo,
    /// Putting items back from the trash (`JobSpec::RestoreFromTrash`).
    Restore,
}

impl JobKind {
    /// The word the log writes for this kind: `copy`, `move`, `delete`, `rename`, `create`,
    /// `link`, `undo` or `restore`. It is English whatever language the UI shows, so logs can be
    /// compared.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Delete => "delete",
            Self::Rename => "rename",
            Self::Create => "create",
            Self::Link => "link",
            Self::Undo => "undo",
            Self::Restore => "restore",
        }
    }
}

impl JobSpec {
    pub fn kind(&self) -> JobKind {
        match self {
            Self::Transfer(spec) => match spec.mode {
                TransferMode::Copy => JobKind::Copy,
                TransferMode::Move => JobKind::Move,
            },
            Self::Delete(_) | Self::DeleteCopies(_) | Self::DeleteFromTrash { .. } => {
                JobKind::Delete
            }
            Self::Rename { .. } => JobKind::Rename,
            Self::Create { .. } => JobKind::Create,
            Self::MakeLink { .. } => JobKind::Link,
            Self::Undo(_) => JobKind::Undo,
            Self::RestoreFromTrash { .. } => JobKind::Restore,
        }
    }

    /// Whether a job of this spec can be undone once it has changed something: every job but
    /// those that delete for good (a permanent delete, a delete of copies, a delete from the
    /// trash), which leave no undo record. A window asks before it submits one of
    /// those, and its question says that it cannot be undone.
    pub fn undoable(&self) -> bool {
        !matches!(
            self,
            Self::Delete(DeleteSpec {
                method: DeleteMethod::Permanent,
                ..
            }) | Self::DeleteCopies(_)
                | Self::DeleteFromTrash { .. }
        )
    }

    /// This spec for a run after one that a crash cut off once it had started: a delete of
    /// copies is [`DeleteCopiesSpec::resumed`]; any other spec is as it is.
    #[must_use]
    pub fn resumed(self) -> JobSpec {
        match self {
            Self::DeleteCopies(spec) => Self::DeleteCopies(DeleteCopiesSpec {
                resumed: true,
                ..spec
            }),
            other => other,
        }
    }

    /// Whether the job writes a journal. A rename or a create is one atomic step and needs
    /// none; a transfer or a delete works through many items one by one and keeps one, and so
    /// does an undo, which may move, copy or trash as many, and a job that makes several
    /// links. A link job leaves no temporary file for recovery to remove; its journal lines let
    /// the next start say that it was cut off. Run again, it numbers the links that were made
    /// already, as a copy under `ConflictPolicy::AutoRename` does. The jobs on the trash keep
    /// none: each item is one rename or one delete, and what a crash cut off is seen in the trash
    /// itself.
    pub fn journaled(&self) -> bool {
        matches!(
            self,
            Self::Transfer(_)
                | Self::Delete(_)
                | Self::DeleteCopies(_)
                | Self::MakeLink { .. }
                | Self::Undo(_)
        )
    }
}
