//! The base: the execution context, reachable by reflection.
//!
//! `p_base` IS the execution context — that is x-lang's model, not a detail of
//! the C engine. A base carries the interpreter's state as a PAIR TYPE, and the
//! library reaches into it by walking the routes the engine commits to in
//! `tools/contract/base-paths.x`.
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
    // --- routes the LIBRARY walks, rooted at the base ---
    // Each is a cell the library reads or writes by name. They are here rather
    // than folded into the engine's own state because the library resolves them
    // at runtime and would die on the first one missing.
    "line",
    "files",
    "profile",
    "false",
    // The REPL's two: the fd currently being read, and the buffer being read
    // from. lib/x/repl/loop.x snapshots the fd per read so a cancelled read can
    // un-poison it, and resets the buffer when a form is abandoned.
    "filein",
    "buffer",
    // --- the evaluator's own state, as the reference keeps it ---
    // The save stack decides what a `def` is; the tco pair is the deferred
    // tail; sigint is the interrupt flag object %sigint-flag also names; the
    // error-handler cell holds the active guard chain.
    crate::vocabulary::ROUTE_SAVE_STACK,
    crate::vocabulary::ROUTE_TCO_EXPR,
    crate::vocabulary::ROUTE_TCO_ENV,
    crate::vocabulary::ROUTE_SIGINT,
    crate::vocabulary::ROUTE_ERROR_HANDLER,
];

/// Slots the heap instructions reach for by name.
pub const TYPE_ALIST: usize = 1;
pub const MARK_HOOKS: usize = 8;
pub const FREE_HOOKS: usize = 9;
pub const MARK_ROOTS: usize = 10;
pub const ALLOC_LIMIT: usize = 11;
pub const ALLOC_COUNT: usize = 12;
pub const LINE: usize = 13;
pub const FILES: usize = 14;
pub const PROFILE: usize = 15;
pub const FALSE: usize = 16;
pub const FILEIN: usize = 17;
pub const BUFFER: usize = 18;
pub const SAVE_STACK: usize = 19;
pub const TCO_EXPR: usize = 20;
pub const TCO_ENV: usize = 21;
pub const SIGINT: usize = 22;
pub const ERROR_HANDLER: usize = 23;

/// Where the environment sits, since the engine reads it constantly.
const ENV_SLOT: usize = 7;
const PRIMS_SLOT: usize = 0;

/// Build a base spine with every route present and nil-valued, then fill the
/// two the engine sets itself.
///
/// Every cell EXISTS even when its value is nil: a route that resolved to
/// nothing would be indistinguishable from a route the engine forgot, and the
/// library's walk would answer nil rather than failing.
/// A spine of `n` cells, each holding a fresh one-word cell the library can read
/// with `%cell-int` and write with `%set-cell-int!`.
///
/// The library treats these slots as CELLS, not as values: `lib/x/sys/stream.x`
/// reads the current output fd with `(first (first (rest (%files))))`, so the
/// slot has to contain something with a `first` to read.
fn cell_spine(o: &mut Objects, n: usize) -> Obj {
    let mut spine = NIL;
    for _ in 0..n {
        let cell = raw_cell(o, 0);
        spine = o.spair(cell, spine);
    }
    spine
}

/// A cell whose first data word IS the number, not a pointer to one.
///
/// `%cell-int` in lib/x/boot/data.x is a RAW WORD READ —
/// `(%ptr-ref-word (%obj->ptr x) %data-off-0)` — so a cell holding an int
/// OBJECT answers that object's ADDRESS. A fresh base's line counter came back
/// as 27021584 instead of 1, and every fd read the same way.
///
/// The library WRITES these too, with `%set-cell-int!`, so the slot has to be a
/// place a bare integer lives rather than a reference to one.
fn raw_cell(o: &mut Objects, n: i64) -> Obj {
    let cell = o.spair(NIL, NIL);
    o.set_data(cell, 0, crate::obj::Word(n as u64));
    cell
}

pub fn build(o: &mut Objects, catalog: Obj, env: EnvId) -> Obj {
    let mut spine = NIL;
    for _ in 0..ROUTES.len() {
        spine = o.spair(NIL, spine);
    }
    let env_obj = o.env_obj(env);
    set_slot(o, spine, PRIMS_SLOT, catalog);
    set_slot(o, spine, ENV_SLOT, env_obj);

    // The library's own slots, shaped the way it reads them.
    // A fresh base's line counter reads 1: the first line of the source is
    // line one, and x-lang's own spec says so.
    let line = raw_cell(o, 1);
    set_slot(o, spine, LINE, line);
    // stdin, stdout, stderr: the library indexes the second and third.
    let files = cell_spine(o, 3);
    set_slot(o, spine, FILES, files);
    // Counters, read through %cell-int. Nine, which is what lib/x/tool/profile.x
    // names; unused ones simply stay zero.
    let profile = cell_spine(o, 9);
    set_slot(o, spine, PROFILE, profile);
    // The false SINGLETON itself, not a cell holding it: x-lang writes into its
    // rest, so the object the route answers must be the one with the room.
    let f = o.false_obj();
    set_slot(o, spine, FALSE, f);
    // A cell holding the fd, so `(%cell-int (first …))` reads it. 0 is stdin,
    // which is where a bare engine's program arrives.
    let fd = raw_cell(o, 0);
    set_slot(o, spine, FILEIN, fd);
    // The buffer slot exists and is empty: the library resets what it finds
    // here, and a route that walked off the end would answer nil either way —
    // indistinguishable from a route this engine forgot.
    set_slot(o, spine, BUFFER, NIL);
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

/// A fresh line counter for a pushed input source, shaped as the library
/// reads it (`%cell-int` on the row's value).
pub fn fresh_line_cell(o: &mut Objects, n: i64) -> Obj {
    raw_cell(o, n)
}

/// Indices into `Objects::state_nodes`.
pub const SN_SAVE: usize = 0;
pub const SN_TCO_EXPR: usize = 1;
pub const SN_TCO_ENV: usize = 2;
pub const SN_HANDLER: usize = 3;

/// Resolve the evaluator-state nodes for a base, for `Objects::state_nodes`.
pub fn state_nodes(o: &Objects, base: Obj) -> [Obj; 4] {
    [
        cell(o, base, SAVE_STACK),
        cell(o, base, TCO_EXPR),
        cell(o, base, TCO_ENV),
        cell(o, base, ERROR_HANDLER),
    ]
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
    /// A base with fewer cells than declared routes walks off the end and
    /// answers nil, which reads as "no value" rather than "no such route".
    #[test]
    fn every_declared_route_resolves() {
        let mut o = Objects::new();
        let base = build(&mut o, NIL, EnvId::from_word(crate::obj::Word(0)));
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
        let base = build(&mut o, NIL, EnvId::from_word(crate::obj::Word(0)));
        let last = cell(&o, base, ROUTES.len() - 1);
        assert!(o.rest(last).is_nil(), "the spine has a spare cell");
    }

    #[test]
    fn the_engine_reads_back_what_it_wrote() {
        let mut o = Objects::new();
        let catalog = o.sym("catalog-stand-in");
        let base = build(&mut o, catalog, EnvId::from_word(crate::obj::Word(3)));
        assert_eq!(catalog_of(&o, base), catalog);
        assert_eq!(env_of(&o, base), EnvId::from_word(crate::obj::Word(3)));
    }

    /// Two bases are distinct spines: writing one must not disturb the other.
    #[test]
    fn bases_do_not_share_cells() {
        let mut o = Objects::new();
        let a = build(&mut o, NIL, EnvId::from_word(crate::obj::Word(1)));
        let b = build(&mut o, NIL, EnvId::from_word(crate::obj::Word(2)));
        assert_eq!(env_of(&o, a), EnvId::from_word(crate::obj::Word(1)));
        assert_eq!(env_of(&o, b), EnvId::from_word(crate::obj::Word(2)));
    }

    /// The ROUTES list and base-paths.x are the same list, and this is what
    /// keeps them so.
    ///
    /// The names live twice — here, beside the slot constants that index them,
    /// and in the contract file x-lang reads. That duplication is deliberate
    /// (a slot constant next to a name read from a file at runtime would be
    /// worse), and it is only safe while something compares them. A route
    /// renamed on one side alone fails here rather than at boot.
    #[test]
    fn the_route_list_is_exactly_what_base_paths_declares() {
        let paths = std::fs::read_to_string("tools/contract/base-paths.x")
            .expect("the engine's own committed paths");
        // Spine rows only: a base-rooted row whose steps are all `r` names a
        // cell of the spine ROUTES builds. A row with an `f` step is DERIVED —
        // it walks INTO a value (env-alist goes through the env object) and
        // owns no spine cell, so it is judged by its own resolution test
        // rather than by this list.
        let rows: Vec<(String, usize, bool)> = paths
            .lines()
            .filter_map(|l| {
                let body = l.trim().strip_prefix('(')?;
                let body = body.split(';').next().unwrap_or("");
                let mut w = body.trim().trim_end_matches(')').split_whitespace();
                let name = w.next()?;
                let root = w.next()?;
                if root != "base" {
                    return None;
                }
                let steps: Vec<&str> = w.collect();
                let rs = steps.iter().take_while(|s| **s == "r").count();
                let clean = steps[rs..].iter().all(|s| *s == "f");
                Some((name.to_string(), rs, clean))
            })
            .collect();
        // Every spine slot's row leads with exactly its index in `r` steps; a
        // trailing `f` run means the row lands on the value instead of the
        // node, and rows through the env object (env-alist) are judged by
        // their own resolution tests.
        for (i, route) in ROUTES.iter().enumerate() {
            let row = rows
                .iter()
                .find(|(n, _, _)| n == route)
                .unwrap_or_else(|| panic!("no base-paths.x row for route {route}"));
            assert!(row.2, "route {route}: steps are not r-run then f-run");
            assert_eq!(row.1, i, "route {route}: r-count disagrees with its slot");
        }
    }

    /// The derived env-alist route lands on the frame chain: seven rests to the
    /// env cell, first into the env object, first into its holder — whose first
    /// is the alist of `(sym . val)` cells. Walked with the same raw first/rest
    /// the library's registry uses, on a REAL engine, so a change to either the
    /// spine or the env representation fails here rather than in a spec.
    #[test]
    fn the_env_alist_route_reaches_the_bindings() {
        let mut e = crate::engine::Engine::new();
        e.eval_str("(def env-alist-probe 77)").unwrap();
        let mut at = e.base;
        for _ in 0..ENV_SLOT {
            at = e.objects.rest(at);
        }
        let holder = {
            let env_obj = e.objects.first(at);
            e.objects.first(env_obj)
        };
        let mut chain = e.objects.first(holder);
        let name = e.objects.sym("env-alist-probe");
        let mut found = false;
        while !chain.is_nil() {
            let pair = e.objects.first(chain);
            if e.objects.first(pair) == name {
                assert_eq!(e.objects.as_int(e.objects.rest(pair)), 77);
                found = true;
                break;
            }
            chain = e.objects.rest(chain);
        }
        assert!(
            found,
            "the bound name is not on the chain the route reaches"
        );
    }
}
