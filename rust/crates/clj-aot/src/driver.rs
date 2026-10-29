// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The real S18 emission driver: [`CompileDriver`] over the
//! plan → module ([`lower`]) → host object ([`target`]) path.
//! Linking never happens here (a library has no entry point), so
//! every failure shape is exactly [`clj_eval`]'s [`DriveError`]
//! minus `Plan` (planning stays caller-side) minus `Link` (CLI-only).

use std::path::Path;

use clj_eval::SubsetProgram;
use clj_eval::compiler::{CompileDriver, DriveError};
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{FileType, TargetMachine};

use crate::{lower, target};

/// The real driver (unit: emission is stateless — parallel-safe).
struct RealDriver;

impl CompileDriver for RealDriver {
    fn emit(
        &self,
        program: &SubsetProgram,
        source_path: &str,
        obj_path: &Path,
    ) -> Result<(), DriveError> {
        let context = Context::create();
        let module = lower::to_module(&context, program, source_path).map_err(DriveError::Emit)?;
        let machine = target::host_machine().map_err(DriveError::Target)?;
        emit_object(&machine, &module, obj_path)
    }
}

static REAL_DRIVER: RealDriver = RealDriver;

/// Install the real driver as `(compile ...)`'s backend (the linking
/// binary calls this at startup; idempotent).
pub fn install_driver() {
    clj_eval::compiler::install_driver(Some(&REAL_DRIVER));
}

/// [`target::emit_object`] with structured errors: the same inkwell
/// calls, but LLVM failures stay `Emit` while file writes become
/// `Io` (the CLI keeps its own conflated mapping untouched).
fn emit_object(
    machine: &TargetMachine,
    module: &Module<'_>,
    obj_path: &Path,
) -> Result<(), DriveError> {
    module.set_triple(&TargetMachine::get_default_triple());
    let layout = machine.get_target_data().get_data_layout();
    module.set_data_layout(&layout);
    let buffer = machine
        .write_to_memory_buffer(module, FileType::Object)
        .map_err(|e| DriveError::Emit(e.to_string()))?;
    std::fs::write(obj_path, buffer.as_slice()).map_err(|e| DriveError::Io {
        path: obj_path.to_path_buf(),
        message: e.to_string(),
    })?;
    Ok(())
}
