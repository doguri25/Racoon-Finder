//! Running a copy or a move: plan it, then transfer the planned items one by one.

use rf_vfs::RenameMode;

use super::file::copy_file;
use super::scan::scan_transfer;
use super::{At, Attempt, Ctx};
use crate::report::Stage;
use crate::spec::{TransferMode, TransferSpec};

/// Plans `spec` and copies each planned item to its target under the job's error policy. A
/// copy never replaces anything: a target that exists when the copy commits fails the item.
pub(crate) fn run_transfer(ctx: &mut Ctx<'_>, spec: &TransferSpec) {
    if spec.mode == TransferMode::Move {
        ctx.not_yet("move is not implemented yet");
        return;
    }
    let Some(plan) = scan_transfer(ctx, spec) else {
        return;
    };
    for planned in &plan.items {
        let (source, target) = (&planned.item.source, &planned.item.target);
        let copied = ctx.attempt(spec.options.errors, Some(source), Some(target), |ctx| {
            // A cancel between items creates nothing more.
            ctx.check().at(Stage::Write)?;
            let src = ctx.provider(source).at(Stage::Read)?;
            let dst = ctx.provider(target).at(Stage::Write)?;
            copy_file(
                ctx,
                &src,
                source,
                &planned.entry,
                &dst,
                target,
                RenameMode::NoReplace,
            )
        });
        match copied {
            Attempt::Done(()) => {
                ctx.item_done(true, false);
                ctx.report.outputs.push(target.clone());
            }
            Attempt::Skipped => {}
            Attempt::Stop => return,
        }
    }
}
