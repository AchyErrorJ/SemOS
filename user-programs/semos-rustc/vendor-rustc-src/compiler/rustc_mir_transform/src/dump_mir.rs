//! This pass just dumps MIR at a specified point.

#[cfg(not(target_os = "none"))]
use std::fs::File;
#[cfg(not(target_os = "none"))]
use std::io;
#[cfg(target_os = "none")]
use semos_std::io;

use rustc_middle::mir::{Body, write_mir_pretty};
use rustc_middle::ty::TyCtxt;
use rustc_session::config::{OutFileName, OutputType};

pub(super) struct Marker(pub &'static str);

impl<'tcx> crate::MirPass<'tcx> for Marker {
    fn name(&self) -> &'static str {
        self.0
    }

    fn run_pass(&self, _tcx: TyCtxt<'tcx>, _body: &mut Body<'tcx>) {}

    fn is_required(&self) -> bool {
        false
    }
}

// M27 §1.3 R4 dump path — host build writes to a real file; SemOS target
// either dumps to stdout (already in-memory) or no-ops on a Real path.
#[cfg(not(target_os = "none"))]
pub fn emit_mir(tcx: TyCtxt<'_>) -> io::Result<()> {
    match tcx.output_filenames(()).path(OutputType::Mir) {
        OutFileName::Stdout => {
            let mut f = io::stdout();
            write_mir_pretty(tcx, None, &mut f)?;
        }
        OutFileName::Real(path) => {
            let mut f = File::create_buffered(&path)?;
            write_mir_pretty(tcx, None, &mut f)?;
            if tcx.sess.opts.json_artifact_notifications {
                tcx.dcx().emit_artifact_notification(&path, "mir");
            }
        }
    }
    Ok(())
}

// M27 §1.3 R4 dump path — SemOS target: emit to stdout when requested,
// no-op when a real file path is asked for (SemOS lacks buffered file
// writes from this layer).
#[cfg(target_os = "none")]
pub fn emit_mir(tcx: TyCtxt<'_>) -> io::Result<()> {
    match tcx.output_filenames(()).path(OutputType::Mir) {
        OutFileName::Stdout => {
            let mut f = io::stdout();
            write_mir_pretty(tcx, None, &mut f)?;
        }
        OutFileName::Real(_path) => {
            // No buffered-file emitter available on SemOS; MIR file dumps
            // become a no-op. Stdout path above is still functional.
        }
    }
    Ok(())
}
