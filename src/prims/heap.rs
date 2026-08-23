//! The heap: counting, collection, and the registration lists.
//!
//! Mirrors the reference engine's `x-prim/heap.c`.
//!
//! **This engine has no collector**, and that is a design consequence rather
//! than an omission: the `core` profile does not include `isa/gc`, so the
//! smallest thing that can boot x-lang allocates into a growing heap and never
//! frees. Implementing this group does not change that — it makes the
//! instructions *reachable*, which is what the capability means.
//!
//! What that costs is nothing the contract asks for. `heap collect` must
//! PRESERVE a reachable object and be idempotent from the caller's view; a heap
//! that never frees satisfies both by construction, and says so here rather than
//! pretending to sweep. If a collector is ever added, `gc/explicit-only` and
//! `gc/non-moving` stop being free and have to be earned — the compliance suite
//! is where that gets falsified.
//!
//! REGISTRATION IS THE ENGINE'S JOB; INVOCATION IS THE LIBRARY'S. The three hook
//! and root operations look like collector internals and are not. x-lang's own
//! note is explicit: a registered callable is "intended to be invoked once per
//! garbage-collection mark phase BY THE CONSUMING LAYER". The engine only puts
//! it on a list, and that list is a base field the layout contract addresses —
//! which is why these are observable without running a collection at all.

use crate::base;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{Obj, NIL};
use crate::prim::PrimDef;

/// `(heap count)` — how many objects have been allocated.
///
/// Must INCREASE across an allocation, which is the whole of what the contract
/// asks. It counts allocations rather than live objects, and on a heap that
/// never frees those are the same number.
fn count(e: &mut Engine, _a: &[Obj]) -> EvalResult {
    let n = e.objects.alloc_count() as i64;
    Ok(e.objects.int(n))
}

/// `(heap collect)` — preserves everything, because nothing is ever freed.
///
/// The two properties the contract states are that a reachable object survives
/// and that collecting twice looks the same as once. Both hold here for the
/// strongest possible reason: there is no collector to get them wrong.
fn collect(_e: &mut Engine, _a: &[Obj]) -> EvalResult {
    Ok(NIL)
}

/// `(heap mark)` and `(heap sweep)` — the halves of a collection this engine
/// does not run.
///
/// x-lang leaves their call shapes undefined and says so; they are here because
/// the capability is the whole group, and an engine that declared `isa/gc` while
/// omitting two rows would be claiming a group it does not cover.
fn mark(_e: &mut Engine, _a: &[Obj]) -> EvalResult {
    Ok(NIL)
}

fn sweep(_e: &mut Engine, _a: &[Obj]) -> EvalResult {
    Ok(NIL)
}

/// `(heap pin! o)` — o must survive collection.
///
/// Answers the object, so a caller can pin inline. Nothing to record: every
/// object here already survives everything.
fn pin(_e: &mut Engine, a: &[Obj]) -> EvalResult {
    Ok(a[0])
}

/// `(heap mark-hook! f)` — prepend to the base's mark-hook list.
fn mark_hook(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let b = e.base;
    base::push(&mut e.objects, b, base::MARK_HOOKS, a[0]);
    Ok(a[0])
}

/// `(heap free-hook! f)` — prepend to the base's free-hook list.
fn free_hook(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let b = e.base;
    base::push(&mut e.objects, b, base::FREE_HOOKS, a[0]);
    Ok(a[0])
}

/// `(heap mark-root! o)` — record an object the collector must treat as
/// reachable whatever else points at it.
///
/// The engine's part is RECORDING it. Whether a later collection honours the
/// list is the collector's behaviour, and this engine's collector is the empty
/// one.
fn mark_root(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let b = e.base;
    base::push(&mut e.objects, b, base::MARK_ROOTS, a[0]);
    Ok(a[0])
}

/// `(alloc-limit! n)` — arm the allocation ceiling.
///
/// Bound BARE as well as filed, precisely so a harness can arm it before
/// anything loads. Every runner in x-lang does, including the conformance one:
/// an engine that filed it only in the catalog would leave every bare harness
/// unable to guard itself.
///
/// It is ENFORCED, not merely recorded. A heap that never frees is exactly the
/// kind that needs a ceiling, and the guard exists because unbounded allocation
/// has taken this project's machine down before.
fn alloc_limit(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let n = e.objects.as_int(a[0]);
    let b = e.base;
    base::set(&mut e.objects, b, base::ALLOC_LIMIT, a[0]);
    e.alloc_limit = if n > 0 { Some(n as usize) } else { None };
    Ok(a[0])
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::filed_full("heap", "count", 0, count),
    PrimDef::both_full("heap-collect", "heap", "collect", 0, collect),
    PrimDef::filed_full("heap", "mark", 0, mark),
    PrimDef::filed_full("heap", "sweep", 0, sweep),
    PrimDef::filed_full("heap", "pin!", 1, pin),
    PrimDef::filed_full("heap", "mark-hook!", 1, mark_hook),
    PrimDef::filed_full("heap", "free-hook!", 1, free_hook),
    PrimDef::filed_full("heap", "mark-root!", 1, mark_root),
    PrimDef::both_full("alloc-limit!", "alloc", "limit!", 1, alloc_limit),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{int_of, raises, truthy, with_coords};

    const HEAP: &[(&str, &str, &str)] = &[
        ("%count", "heap", "count"),
        ("%collect", "heap", "collect"),
        ("%pin", "heap", "pin!"),
        ("%mkhook", "heap", "mark-hook!"),
        ("%fhook", "heap", "free-hook!"),
        ("%mkroot", "heap", "mark-root!"),
    ];

    fn heap(body: &str) -> String {
        with_coords(HEAP, body)
    }

    /// The contract asks only that it INCREASE across an allocation.
    #[test]
    fn the_count_rises_when_something_is_allocated() {
        assert!(truthy(&heap(
            "(def before (%count)) (def junk (pair (pair 1 2) (pair 3 4)))
             (< before (%count))"
        )));
    }

    /// The one property that matters, and the one an engine free to collect
    /// what is still referenced would fail.
    #[test]
    fn collection_preserves_a_reachable_object() {
        assert!(truthy(&heap(
            "(def keep (pair 11 22)) (%collect)
             (match ((= (first keep) 11) (= (rest keep) 22)) (#t ()))"
        )));
    }

    #[test]
    fn collection_is_idempotent_from_the_callers_view() {
        assert_eq!(
            int_of(&heap(
                "(def keep (pair 33 44)) (%collect) (%collect) (first keep)"
            )),
            33
        );
    }

    #[test]
    fn a_pinned_object_survives_and_is_answered_back() {
        assert_eq!(
            int_of(&heap("(def p (pair 55 66)) (%pin p) (%collect) (first p)")),
            55
        );
        assert!(truthy(&heap("(def p (pair 1 2)) (same? (%pin p) p)")));
    }

    /// Bound BARE, not merely filed: a harness arms it before anything loads.
    #[test]
    fn the_allocation_ceiling_is_armable_without_a_library() {
        assert!(truthy("(match ((eq? alloc-limit! ()) ()) (#t 1))"));
    }

    /// And it is ENFORCED. A heap that never frees is the kind that needs a
    /// ceiling; recording the number without honouring it would be a guard in
    /// name only.
    #[test]
    fn the_allocation_ceiling_is_enforced() {
        assert!(
            raises("(alloc-limit! 1) (def burn (fn (self n) (match ((= n 0) 1) (#t (%seq (pair n n) (self (- n 1))))))) (burn 1000)"),
            "allocating past the ceiling must raise"
        );
    }

    /// PREPEND, and the cases read the head to check it — appending would pass a
    /// test that only counted.
    #[test]
    fn the_hooks_prepend_to_their_lists() {
        for (reg, route) in [
            ("%mkhook", "heap-mark-hooks"),
            ("%fhook", "heap-free-hooks"),
            ("%mkroot", "heap-mark-roots"),
        ] {
            let src = heap(&format!(
                "(def %f (fn (_) 1)) ({} %f)
                 (same? (first (first (%walk (rest (rest (%assoc (lit {}) %base-paths))) (%base)))) %f)",
                reg, route
            ));
            assert!(truthy(&src), "{} did not prepend to {}", reg, route);
        }
    }
}
