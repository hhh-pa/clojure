// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! LLVM AOT backend for the pure Clojure subset.
//!
//! The compiler itself lands in slice S8 (`docs/t1174/`
//! `specification.md` 2.4). Slice S1 only links the toolchain and
//! proves it with [`probe_ir`].

use inkwell::context::Context;

/// Probe that the LLVM backend is linked: build an empty module and
/// render its IR. The `llvm22-1` feature guarantees the LLVM major.
#[must_use]
pub fn probe_ir() -> String {
    let context = Context::create();
    let module = context.create_module("probe");
    module.print_to_string().to_string()
}

#[cfg(test)]
mod tests {
    use super::probe_ir;

    #[test]
    fn probe_renders_module_ir() {
        let ir = probe_ir();
        assert!(ir.contains("ModuleID"), "{ir}");
    }
}
