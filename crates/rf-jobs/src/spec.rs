//! What a job is asked to do: the data the UI hands to the engine, before any file is touched.

use std::ffi::OsString;
use std::time::SystemTime;

use rf_core::{Error, Result, VPath};
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
    /// Reverses an earlier job as its record says (OPS-12). The report's undo record reverses
    /// this job in turn, which is how a redo is done.
    Undo(UndoRecord),
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
    Undo,
}

impl JobKind {
    /// The word the log writes for this kind: `copy`, `move`, `delete`, `rename`, `create` or
    /// `undo`. It is English whatever language the UI shows, so logs can be compared.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Delete => "delete",
            Self::Rename => "rename",
            Self::Create => "create",
            Self::Undo => "undo",
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
            Self::Delete(_) | Self::DeleteCopies(_) => JobKind::Delete,
            Self::Rename { .. } => JobKind::Rename,
            Self::Create { .. } => JobKind::Create,
            Self::Undo(_) => JobKind::Undo,
        }
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
    /// does an undo, which may move, copy or trash as many.
    pub fn journaled(&self) -> bool {
        matches!(
            self,
            Self::Transfer(_) | Self::Delete(_) | Self::DeleteCopies(_) | Self::Undo(_)
        )
    }
}
