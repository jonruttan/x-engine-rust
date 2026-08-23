//! The instruction set, as tables.
//!
//! ONE MODULE PER SUBJECT, not per capability group. The capability tags in
//! `tools/contract/isa.x` cut across subjects — `str append` is tagged `alloc`
//! and `str byte-len` is tagged `hot` — so a module per tag would split the
//! string code in half and put two halves of one idea in different files. The
//! tag is a fact about the contract and lives in the contract; the module is a
//! fact about the code and follows what the code touches.
//!
//! Every module exports a `&[PrimDef]` and its own `#[cfg(test)] mod tests`. A
//! primitive is a plain function over evaluated arguments, so a test calls it
//! through `Engine::eval_str` and asserts on the object — no subprocess, no
//! conformance runner, no counting failures to find out something broke.

use crate::prim::PrimDef;

pub mod base;
pub mod callcc;
pub mod core;
pub mod ffi;
pub mod heap;
pub mod io;
pub mod iter;
pub mod num;
pub mod obj;
pub mod ptr;
pub mod str;
pub mod sys;
pub mod tok;

/// Every instruction this engine implements, in one list. `Engine::new` walks it
/// once, binding bare names and filing coordinates from the SAME row so the two
/// cannot disagree.
pub fn all() -> Vec<PrimDef> {
    let mut v = Vec::new();
    for table in [
        crate::syntax::binding::TABLE,
        crate::syntax::closure::TABLE,
        crate::syntax::control::TABLE,
        crate::syntax::quote::TABLE,
        core::TABLE,
        base::TABLE,
        callcc::TABLE,
        heap::TABLE,
        sys::TABLE,
        ffi::TABLE,
        num::TABLE,
        obj::TABLE,
        str::TABLE,
        ptr::TABLE,
        iter::TABLE,
        io::TABLE,
        tok::TABLE,
    ] {
        v.extend_from_slice(table);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// No instruction may be registered twice under the same name or the same
    /// coordinate. Two rows binding one name is a silent shadow — the later wins
    /// and the earlier is unreachable — and nothing else in the system would
    /// notice, because the manifest lists the name once either way.
    #[test]
    fn no_duplicate_registrations() {
        let mut names = HashSet::new();
        let mut coords = HashSet::new();
        for def in all() {
            if let Some(n) = def.bare {
                assert!(names.insert(n), "bare name {} registered twice", n);
            }
            if let Some(c) = def.coord {
                assert!(coords.insert(c), "coordinate {:?} registered twice", c);
            }
        }
    }

    /// Every row must be reachable somehow. A row with neither a bare name nor a
    /// coordinate is dead weight that still consumes a prim index.
    #[test]
    fn every_row_is_reachable() {
        for def in all() {
            assert!(
                def.bare.is_some() || def.coord.is_some(),
                "a row is reachable by neither name nor coordinate"
            );
        }
    }
}
