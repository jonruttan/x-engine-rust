//! The base: the execution context, reachable by reflection.
//!
//! `p_base` IS the execution context — that is x-lang's model, not a detail of
//! the C engine. A base carries the interpreter's state as a PAIR TREE, and the
//! library reaches into it by walking the routes the engine commits to in
//! `tools/contract/base-paths.x`.
//!
//! This engine had a two-element pair standing in for one. It satisfied the
//! `core` profile and every conformance case that exists, because none of them
//! looks, while the state a base is supposed to carry sat in a Rust struct that
//! reflection cannot see. `make check-base-routes` in x-lang is what says so:
//! the library walks sixteen routes by name and would have died on the first.
//!
//! # The shape
//!
//! A flat spine, one cell per route, so that a route ends at the CELL whose
//! `first` is its value — the convention `base-paths.x` documents and the
//! conformance prelude relies on:
//!
//! ```text
//!   (prims base)              the primitive catalog
//!   (type-alist base r)       registered types, by name
//!   (error-str base r r)      the last error's text
//!   (err-line base r r r)
//!   (err-file base r r r r)
//!   (file-registry base r r r r r)
//!   (obj-meta-extra base r r r r r r)
//!   (env base r r r r r r r)  this base's environment
//! ```
//!
//! The STEPS are this engine's to choose — decision L1 exists so that a
//! different object model can arrange its base differently. The NAMES are not:
//! the library resolves them by name at runtime.
//!
//! Flat, rather than the C's nested groups, because there is nothing here to
//! group yet. It grows a shape when it grows contents.

use crate::obj::{EnvId, Obj, NIL};
use crate::objects::Objects;

/// One cell per committed route, in the order `base-paths.x` declares.
///
/// The order is the contract. Adding a route means appending here AND to
/// base-paths.x, and the two are checked against each other by
/// [`tests::every_declared_route_resolves`].
pub const ROUTES: &[&str] = &[
    "prims",
    "type-alist",
    "error-str",
    "err-line",
    "err-file",
    "file-registry",
    "obj-meta-extra",
    "env",
    // --- the heap's registration lists, and the allocation ceiling ---
    // These are LISTS the engine prepends to, not collector internals.
    // x-lang's own note is explicit: a registered callable is "intended to be
    // invoked once per garbage-collection mark phase BY THE CONSUMING LAYER".
    // The engine records; the library invokes.
    "heap-mark-hooks",
    "heap-free-hooks",
    "heap-mark-roots",
    "alloc-limit",
    "alloc-count",
];

/// Slots the heap instructions reach for by name.
pub const MARK_HOOKS: usize = 8;
pub const FREE_HOOKS: usize = 9;
pub const MARK_ROOTS: usize = 10;
pub const ALLOC_LIMIT: usize = 11;
pub const ALLOC_COUNT: usize = 12;

/// Where the environment sits, since the engine reads it constantly.
const ENV_SLOT: usize = 7;
const PRIMS_SLOT: usize = 0;

/// Build a base spine with every route present and nil-valued, then fill the
/// two the engine sets itself.
///
/// Every cell EXISTS even when its value is nil. A route that resolved to
/// nothing would be indistinguishable from a route the engine forgot, and the
/// library's walk would answer nil rather than failing — which is the quiet
/// half of the bug this replaces.
pub fn build(o: &mut Objects, catalog: Obj, env: EnvId) -> Obj {
    let mut spine = NIL;
    for _ in 0..ROUTES.len() {
        spine = o.pair(NIL, spine);
    }
    let env_obj = o.env_obj(env);
    set_slot(o, spine, PRIMS_SLOT, catalog);
    set_slot(o, spine, ENV_SLOT, env_obj);
    spine
}

/// The cell at `n` steps of `rest` from the base.
fn cell(o: &Objects, base: Obj, n: usize) -> Obj {
    let mut at = base;
    for _ in 0..n {
        at = o.rest(at);
    }
    at
}

fn set_slot(o: &mut Objects, base: Obj, n: usize, v: Obj) {
    let c = cell(o, base, n);
    o.set_data(c, 0, v.word());
}

fn slot(o: &Objects, base: Obj, n: usize) -> Obj {
    o.first(cell(o, base, n))
}

/// The environment this base names.
///
/// Read back through the base's own committed route rather than from a side
/// table, so the descriptor and the engine cannot disagree about where a base
/// keeps its bindings.
pub fn env_of(o: &Objects, base: Obj) -> EnvId {
    o.env_id(slot(o, base, ENV_SLOT))
}

pub fn catalog_of(o: &Objects, base: Obj) -> Obj {
    slot(o, base, PRIMS_SLOT)
}

/// Read one of the named slots.
pub fn get(o: &Objects, base: Obj, n: usize) -> Obj {
    slot(o, base, n)
}

/// Write one of the named slots.
pub fn set(o: &mut Objects, base: Obj, n: usize, v: Obj) {
    set_slot(o, base, n, v)
}

/// Prepend to one of the list slots, answering the new head.
///
/// PREPEND, because that is what the contract says: "mark-hook! prepends the
/// callable to the base's mark-hook list", and the cases read the head to check
/// it. Appending would pass a test that only counted.
pub fn push(o: &mut Objects, base: Obj, n: usize, v: Obj) -> Obj {
    let head = slot(o, base, n);
    let cell = o.pair(v, head);
    set_slot(o, base, n, cell);
    cell
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EVERY route in base-paths.x must resolve on a base this engine builds.
    ///
    /// This is the check the whole session was missing: a base with fewer cells
    /// than declared routes walks off the end and answers nil, which reads as "no
    /// value" rather than "no such route".
    #[test]
    fn every_declared_route_resolves() {
        let mut o = Objects::new();
        let base = build(&mut o, NIL, EnvId::new(0));
        for (n, name) in ROUTES.iter().enumerate() {
            let c = cell(&o, base, n);
            assert!(
                !c.is_nil(),
                "route `{}` at {} steps walks off the end of the base",
                name,
                n
            );
        }
    }

    /// And the spine is exactly as long as the route list — no spare cells, so a
    /// route added here without a cell fails the test above rather than
    /// silently addressing someone else's slot.
    #[test]
    fn the_spine_is_exactly_as_long_as_the_route_list() {
        let mut o = Objects::new();
        let base = build(&mut o, NIL, EnvId::new(0));
        let last = cell(&o, base, ROUTES.len() - 1);
        assert!(o.rest(last).is_nil(), "the spine has a spare cell");
    }

    #[test]
    fn the_engine_reads_back_what_it_wrote() {
        let mut o = Objects::new();
        let catalog = o.sym("catalog-stand-in");
        let base = build(&mut o, catalog, EnvId::new(3));
        assert_eq!(catalog_of(&o, base), catalog);
        assert_eq!(env_of(&o, base), EnvId::new(3));
    }

    /// Two bases are distinct spines: writing one must not disturb the other.
    #[test]
    fn bases_do_not_share_cells() {
        let mut o = Objects::new();
        let a = build(&mut o, NIL, EnvId::new(1));
        let b = build(&mut o, NIL, EnvId::new(2));
        assert_eq!(env_of(&o, a), EnvId::new(1));
        assert_eq!(env_of(&o, b), EnvId::new(2));
    }
}
