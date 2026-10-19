// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Clojure value model: [`Value`] plus [`Symbol`] and [`Keyword`].
//!
//! Values are immutable and `Send + Sync`, shared by `Arc`. Equality
//! is Clojure `=` and hashing is equiv-consistent (see
//! [`value`](crate::value)); printing lives in [`print`](crate::print).

mod atom;
mod compare;
mod delay;
mod extension;
pub mod hasheq;
mod keyword;
mod print;
mod seq;
mod symbol;
mod transient;
mod value;

pub use atom::{Atom, AtomState};
pub use compare::{CompareError, compare};
pub use delay::{Delay, DelayState};
pub use extension::{ExtensionValue, ThrowClass, Throwable, Unbound, Var};
pub use hasheq::{hash_code, hasheq, seq_hash_unsupported};
pub use keyword::Keyword;
pub use print::{
    java_debug, java_double_string, pr_to_string, print, print_opts::PrintOpts, print_to_string,
};
pub use seq::{
    CHUNK_SIZE, ConsValue, CycleValue, IterateValue, KeySeqValue, RangeNum, RangeValue,
    RealizedSeq, RepeatValue, Seq, SeqKind, SubVectorValue, ValSeqValue, seq_vec,
};
pub use symbol::Symbol;
pub use transient::{
    Transient, TransientError, TransientKind, TransientMap, TransientSet, TransientVector,
};
pub use value::{FxBuildHasher, List, Map, Set, Value, Vector, new_map, new_set};
