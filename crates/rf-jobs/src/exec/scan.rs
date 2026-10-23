//! The plan of a transfer: what each source is, whether its target folder exists, and how
//! much the job will copy.

use std::collections::HashSet;
use std::io;

use rf_core::{Entry, EntryKind, Error, VPath};

use super::{At, Attempt, Ctx, Failed};
use crate::report::{Stage, Totals};
use crate::spec::{TransferItem, TransferSpec};

/// One item the plan keeps, with what the source was when it was planned.
#[derive(Debug)]
pub(crate) struct Planned {
    pub item: TransferItem,
    /// The source as `stat` saw it (a link is described itself, not what it leads to).
    pub entry: Entry,
    /// A move expected to end with one rename, so its tree is not walked.
    #[expect(
        dead_code,
        reason = "unread until moves are implemented, which must remove this"
    )]
    pub rename: bool,
}

/// What a transfer will do.
#[derive(Debug)]
pub(crate) struct Plan {
    pub items: Vec<Planned>,
    pub totals: Totals,
    /// Targets the pre-flight review left out; each fails as `Stage::Preflight`.
    #[expect(
        dead_code,
        reason = "unread until the pre-flight review is implemented, which must remove this"
    )]
    pub blocked: HashSet<VPath>,
    /// A move that copies, because a source location cannot be written to.
    #[expect(
        dead_code,
        reason = "unread until moves are implemented, which must remove this"
    )]
    pub moved_as_copy: bool,
}

/// Plans `spec`: `stat`s each source (a link itself, not what it leads to) and checks that its
/// target folder exists, under the job's error policy. A file or a link counts as one file, a
/// file with its size and a link with none. A FIFO, socket or device stays in the plan
/// uncounted and is never opened: the run reports it as failed. A folder source fails as not
/// implemented yet. Announces the totals at the end; `None` when the job was cancelled or
/// aborted.
pub(crate) fn scan_transfer(ctx: &mut Ctx<'_>, spec: &TransferSpec) -> Option<Plan> {
    let mut plan = Plan {
        items: Vec::with_capacity(spec.items.len()),
        totals: Totals::default(),
        blocked: HashSet::new(),
        moved_as_copy: false,
    };
    for item in &spec.items {
        let source = &item.source;
        let target = &item.target;
        match ctx.attempt(spec.options.errors, Some(source), Some(target), |ctx| {
            scan_item(ctx, item)
        }) {
            Attempt::Done(entry) => {
                match entry.kind {
                    EntryKind::File => {
                        plan.totals.files += 1;
                        plan.totals.bytes += entry.size;
                    }
                    EntryKind::Symlink => plan.totals.files += 1,
                    EntryKind::Dir | EntryKind::Other => {}
                }
                plan.items.push(Planned {
                    item: item.clone(),
                    entry,
                    rename: false,
                });
            }
            Attempt::Skipped => {}
            Attempt::Stop => return None,
        }
    }
    ctx.planned(plan.totals);
    Some(plan)
}

/// The source entry of `item`, once its target folder is known to exist.
fn scan_item(ctx: &mut Ctx<'_>, item: &TransferItem) -> Result<Entry, Failed> {
    ctx.check().at(Stage::Plan)?;
    let entry = ctx
        .provider(&item.source)
        .and_then(|src| src.stat(&item.source))
        .at(Stage::Plan)?;
    if entry.kind == EntryKind::Dir {
        let error = Error::Unsupported("folders are not implemented yet".to_owned());
        return Err(Failed::new(Stage::Plan, error));
    }
    check_target_folder(ctx, &item.target).at(Stage::Plan)?;
    Ok(entry)
}

/// Fails with `NotFound` unless the folder `target` goes into exists. A link to a folder
/// counts as a folder (macOS `/tmp`, a Windows junction).
fn check_target_folder(ctx: &Ctx<'_>, target: &VPath) -> rf_core::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| Error::Unsupported(format!("no folder to copy into: {target}")))?;
    let dst = ctx.provider(&parent)?;
    let mut folder = dst.stat(&parent)?;
    if folder.kind == EntryKind::Symlink {
        folder = dst.stat_target(&parent)?;
    }
    if folder.kind == EntryKind::Dir {
        Ok(())
    } else {
        let error = io::Error::new(io::ErrorKind::NotFound, "not a folder");
        Err(Error::io("stat", parent, error))
    }
}
