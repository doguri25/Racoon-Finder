//! What a job is asked to do: the data the UI hands to the engine, before any file is touched.

use std::ffi::OsString;

use rf_core::{Error, Result, VPath};
use serde::{Deserialize, Serialize};

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
    /// join error when a name cannot be appended to `dest_dir`.
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

/// A request to the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum JobSpec {
    Transfer(TransferSpec),
    Delete(DeleteSpec),
    Rename {
        from: VPath,
        to: VPath,
    },
    Create {
        parent: VPath,
        name: OsString,
        kind: CreateKind,
    },
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

impl JobSpec {
    pub fn kind(&self) -> JobKind {
        match self {
            Self::Transfer(spec) => match spec.mode {
                TransferMode::Copy => JobKind::Copy,
                TransferMode::Move => JobKind::Move,
            },
            Self::Delete(_) => JobKind::Delete,
            Self::Rename { .. } => JobKind::Rename,
            Self::Create { .. } => JobKind::Create,
        }
    }

    /// Whether the job writes a journal. A rename or a create is one atomic step and needs
    /// none; a transfer or a delete works through many items one by one and keeps one.
    pub fn journaled(&self) -> bool {
        matches!(self, Self::Transfer(_) | Self::Delete(_))
    }
}
