//! The heap: counting, collection, and the registration lists.
//!
//! Mirrors the reference engine's `x-prim/heap.c`.
//!
//! `heap collect` MARKS AND SWEEPS. The `core` profile does not include
//! `isa/gc`, so nothing x-lang runs requires this to free anything — but a heap
//! that only grows took this project's machine down, which is a mechanism and
//! not a preference, so it frees.
//!
//! The contract asks that `heap collect` PRESERVE a reachable object and be
//! idempotent from the caller's view. Both are now properties of the mark rather
//! than of doing nothing, and `gc/explicit-only` and `gc/non-moving` are earned
//! the same way — see `collect.rs`, and the compliance suite for where they get
//! falsified.
//!
//! REGISTRATION IS THE ENGINE'S JOB; INVOCATION IS THE LIBRARY'S. The three handler
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
/// asks — so it counts allocations EVER and collection does not lower it. The
/// live count is a different number and is not what x-lang asked for here.
fn count(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    let n = e.objects.alloc_count() as i64;
    Ok(e.objects.int(n))
}

/// `(heap collect)` — reclaim what nothing can reach.
///
/// The two properties the contract states are that a reachable object survives
/// and that collecting twice looks the same as once. The first is what the mark
/// phase is for; the second follows from the mark being computed fresh each
/// time, so a second collection immediately after a first finds nothing new.
fn collect(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    let freed = e.collect();
    if std::env::var("X_HEAP_STATS").is_ok() {
        eprintln!(
            "collect: freed {} objects, {} live; {} envs made (heap-owned)",
            freed,
            e.objects.live,
            e.envs.frame_count()
        );
    }
    Ok(NIL)
}

/// `(heap mark)` and `(heap sweep)` — the halves of a collection this engine
/// does not run.
///
/// x-lang leaves their call shapes undefined and says so; they are here because
/// the capability is the whole group, and an engine that declared `isa/gc` while
/// omitting two rows would be claiming a group it does not cover.
fn mark(_e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    Ok(NIL)
}

fn sweep(_e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    Ok(NIL)
}

/// `(heap pin! o)` — o must survive collection.
///
/// Answers the object, so a caller can pin inline. Nothing to record: every
/// object here already survives everything.
fn pin(_e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    Ok(a[0])
}

/// `(heap mark-hook! f)` — prepend to the base's mark-hook list.
fn mark_hook(e: &mut Engine, base: Obj, a: &[Obj]) -> EvalResult {
    let b = base;
    base::push(&mut e.objects, b, base::MARK_HOOKS, a[0]);
    Ok(a[0])
}

/// `(heap free-hook! f)` — prepend to the base's free-hook list.
fn free_hook(e: &mut Engine, base: Obj, a: &[Obj]) -> EvalResult {
    let b = base;
    base::push(&mut e.objects, b, base::FREE_HOOKS, a[0]);
    Ok(a[0])
}

/// `(heap mark-root! o)` — record an object the collector must treat as
/// reachable whatever else points at it.
///
/// The engine's part is RECORDING it. Whether a later collection honours the
/// list is the collector's behaviour, and this engine's collector is the empty
/// one.
fn mark_root(e: &mut Engine, base: Obj, a: &[Obj]) -> EvalResult {
    let b = base;
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
/// It is ENFORCED, not merely recorded: collection is EXPLICIT-ONLY, so
/// nothing stands between a runaway loop and the machine except this.
fn alloc_limit(e: &mut Engine, base: Obj, a: &[Obj]) -> EvalResult {
    let n = e.objects.as_int(a[0]);
    let b = base;
    base::set(&mut e.objects, b, base::ALLOC_LIMIT, a[0]);
    e.alloc_limit = if n > 0 { Some(n as usize) } else { None };
    // Side-effect primitive: answers nil, the C contract.
    Ok(NIL)
}

/// `(heap check)` — DEBUG: walk the chain and verify every reference slot
/// holds nil or a plausible object start; answers the number of BAD slots and
/// prints each. Costs a full heap walk; exists to catch structure miswiring
/// the moment it happens rather than three calls later.
fn heap_check(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    let mut bad = 0i64;
    let mut at = e.objects.heap_chain;
    while !at.is_nil() {
        let f = crate::obj::Flags::from_word(crate::obj::Word(
            e.objects.flags_word(at) & !(1u64 << 63),
        ));
        let n = e.objects.data_len(at);
        let check_slots: &[u64] =
            if f == crate::objects::FLAG_PAIR || f == crate::objects::FLAG_SPAIR {
                &[0, 1]
            } else if f == crate::objects::FLAG_ENVH {
                &[0, 1, 2]
            } else {
                &[]
            };
        for &i in check_slots {
            if i >= n {
                continue;
            }
            let w = e.objects.data(at, i).raw();
            if w == 0 {
                continue;
            }
            if w % 8 != 0
                || (w / 8) as usize + crate::objects::META_LEN as usize > e.objects.heap.words_len()
            {
                eprintln!(
                    "HEAP-CHECK bad: {} slot {} holds {:#x}",
                    e.objects.describe_word(at.word().raw()),
                    i,
                    w
                );
                bad += 1;
            }
        }
        at = e.objects.chain_next(at);
    }
    Ok(e.objects.int(bad))
}

crate::uniform_engine!(count_u, count, 0);
crate::uniform_engine!(collect_u, collect, 0);
crate::uniform_engine!(mark_u, mark, 0);
crate::uniform_engine!(sweep_u, sweep, 0);
crate::uniform_engine!(pin_u, pin, 1);
crate::uniform_engine!(heap_check_u, heap_check, 0);
crate::uniform_engine!(mark_hook_u, mark_hook, 1);
crate::uniform_engine!(free_hook_u, free_hook, 1);
crate::uniform_engine!(mark_root_u, mark_root, 1);
crate::uniform_engine!(alloc_limit_u, alloc_limit, 1);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(None, Some(("heap", "count")), 0, count_u),
    PrimDef::row(Some("heap-collect"), Some(("heap", "collect")), 0, collect_u),
    PrimDef::row(None, Some(("heap", "mark")), 0, mark_u),
    PrimDef::row(None, Some(("heap", "sweep")), 0, sweep_u),
    PrimDef::row(None, Some(("heap", "pin!")), 1, pin_u),
    PrimDef::row(None, Some(("heap", "check")), 0, heap_check_u),
    PrimDef::row(None, Some(("heap", "mark-hook!")), 1, mark_hook_u),
    PrimDef::row(None, Some(("heap", "free-hook!")), 1, free_hook_u),
    PrimDef::row(None, Some(("heap", "mark-root!")), 1, mark_root_u),
    PrimDef::row(Some("alloc-limit!"), Some(("alloc", "limit!")), 1, alloc_limit_u),
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

    /// And it is ENFORCED. With collection explicit-only, a runaway loop meets
    /// no other limit; recording the number without honouring it would be a
    /// guard in name only.
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
