// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Host target machine + object emission (S8C).
//!
//! Host-only: x86/aarch64 init (never `target-all`), the default
//! triple, generic CPU, PIC reloc (position-independent objects
//! link everywhere, including macOS ARM64 PIE binaries).

use std::path::Path;

use inkwell::OptimizationLevel;
use inkwell::module::Module;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

/// Build the host target machine (fails only when LLVM itself is
/// broken — the host triple always resolves after x86/aarch64
/// init on every supported runner).
pub fn host_machine() -> Result<TargetMachine, String> {
    Target::initialize_x86(&InitializationConfig::default());
    Target::initialize_aarch64(&InitializationConfig::default());
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::Default,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| format!("no machine for {}", triple.as_str().to_string_lossy()))
}

/// Emit `module` as a native object file (stamps the module triple
/// and data layout from the machine first).
pub fn emit_object(
    machine: &TargetMachine,
    module: &Module,
    obj_path: &Path,
) -> Result<(), String> {
    module.set_triple(&TargetMachine::get_default_triple());
    let layout = machine.get_target_data().get_data_layout();
    module.set_data_layout(&layout);
    let buffer = machine
        .write_to_memory_buffer(module, FileType::Object)
        .map_err(|e| e.to_string())?;
    std::fs::write(obj_path, buffer.as_slice()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{emit_object, host_machine};
    use inkwell::context::Context;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("clj-aot-target-{}-{name}", std::process::id()))
    }

    #[test]
    fn host_machine_emits_objects() {
        let machine = host_machine().expect("host machine");
        let ctx = Context::create();
        let module = ctx.create_module("empty");
        let obj = tmp("empty.o");
        emit_object(&machine, &module, &obj).expect("emit");
        let bytes = std::fs::read(&obj).expect("read back");
        assert!(!bytes.is_empty(), "object must not be empty");
        std::fs::remove_file(&obj).expect("cleanup");
    }

    #[test]
    fn emit_reports_unwritable_paths() {
        let machine = host_machine().expect("host machine");
        let ctx = Context::create();
        let module = ctx.create_module("empty");
        let obj = tmp("no-such-dir").join("x.o");
        assert!(emit_object(&machine, &module, &obj).is_err());
    }
}
