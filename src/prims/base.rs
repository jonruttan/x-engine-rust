//! Bases: another interpreter context, not another environment.
//!
//! Mirrors the reference engine's `x-prim/base.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::Obj;
use crate::prim::PrimDef;

/// `(base make)` — another interpreter context, not another environment.
///
/// The distinction is x-lang's isolation story: a child base is born with the
/// instruction set and NOTHING of the host's, so `(+ 2 3)` works inside it while
/// a name the host defined does not resolve.
fn base_make(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    Ok(e.make_base())
}

/// `(base eval B expr)` — evaluate inside another base.
fn base_eval(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let env = e.base_env(a[0]);
    let (base, form) = (a[0], a[1]);
    // The FORM is not translated. It carries whatever symbols the caller read,
    // and they resolve because instruction names are shared; anything the child
    // interns while running goes into the child's own table.
    e.in_base(base, |e| e.eval(form, env))
}

/// `(base bind B name value)` — the capability-handing door.
///
/// A host decides what a child can see by binding it, one name at a time. What
/// makes it a capability model rather than a naming convenience is that a name
/// bound into one base is unbound in another: bases are rootless, so nothing is
/// shared unless it was handed over.
fn base_bind(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let env = e.base_env(a[0]);
    e.envs.bind(env, a[1], a[2]);
    Ok(a[2])
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::both_full("make-base", "base", "make", 0, base_make),
    PrimDef::filed_full("base", "eval", 2, base_eval),
    PrimDef::filed_full("base", "bind", 3, base_bind),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{int_of, raises, truthy, with_coords};

    const BASES: &[(&str, &str, &str)] = &[
        ("%mkb", "base", "make"),
        ("%beval", "base", "eval"),
        ("%bind", "base", "bind"),
    ];

    fn bases(body: &str) -> String {
        with_coords(BASES, body)
    }

    /// A fresh base is born knowing the MACHINE. A sandbox withholds the host's
    /// definitions, not the instruction set.
    #[test]
    fn a_fresh_base_can_evaluate() {
        assert_eq!(int_of(&bases("(%beval (%mkb) (lit (+ 2 3)))")), 5);
    }

    /// The property that makes it a sandbox rather than a second environment: a
    /// name defined out here is unbound in there. A child frame made with `push`
    /// instead of `push_root` would inherit everything and this would pass 99.
    #[test]
    fn a_sandbox_does_not_see_the_hosts_bindings() {
        assert!(raises(&bases(
            "(def host-only 99) (%beval (%mkb) (lit host-only))"
        )));
    }

    /// And the host cannot see into the child either.
    #[test]
    fn the_host_does_not_see_the_sandboxs_bindings() {
        assert!(raises(&bases(
            "(def b (%mkb)) (%beval b (lit (def inside 1))) inside"
        )));
    }

    /// The capability door: a host decides what a child sees, one name at a time.
    #[test]
    fn a_name_bound_into_a_base_stays_there() {
        assert_eq!(
            int_of(&bases(
                "(def b (%mkb)) (%bind b (lit answer) 42) (%beval b (lit answer))"
            )),
            42
        );
    }

    /// What makes it a capability rather than a naming convenience: bases are
    /// rootless, so nothing is shared unless it was handed over.
    #[test]
    fn a_name_bound_into_one_base_is_unbound_in_another() {
        assert!(raises(&bases(
            "(def b1 (%mkb)) (def b2 (%mkb))
             (%bind b1 (lit answer) 42)
             (%beval b2 (lit answer))"
        )));
    }

    // --- per-base interning --------------------------------------------------
    // Derived by ASKING x-engine-c, not by reading it. Two hypotheses died on
    // the way: that the boundary re-interns the form, and that a child takes a
    // snapshot of its parent's table.

    /// The same spelling on either side of a base boundary is two DIFFERENT
    /// objects. That is what makes `base make` an isolation boundary rather
    /// than a second environment: a host cannot smuggle a name into a child by
    /// constructing it.
    #[test]
    fn a_symbol_interned_in_a_child_is_the_childs_own() {
        assert!(truthy(&bases(
            r#"(def %s2y (%coord (lit str) (lit ->sym)))
               (def b (%mkb))
               (%bind b (lit mk) %s2y)
               (match ((eq? (%s2y "zed") (%beval b (lit (mk "zed")))) ()) (#t 1))"#
        )));
    }

    /// NOT a snapshot taken at `base make`: a symbol the host interned BEFORE
    /// the child existed is still a different object inside it.
    #[test]
    fn interning_in_the_host_first_does_not_share_it() {
        assert!(truthy(&bases(
            r#"(def %s2y (%coord (lit str) (lit ->sym)))
               (def before (%s2y "beforehand"))
               (def b (%mkb))
               (%bind b (lit mk) %s2y)
               (%bind b (lit same) (%coord (lit obj) (lit eq?)))
               (%bind b (lit host-one) before)
               (match ((%beval b (lit (same (mk "beforehand") host-one))) ()) (#t 1))"#
        )));
    }

    /// THE EXCEPTION that makes the rest work: an instruction's name is the
    /// same object in every base, so a form read in the host evaluates in a
    /// child at all. Without it, per-base interning plus identity lookup would
    /// leave every cross-base form unbound.
    #[test]
    fn instruction_names_are_shared_across_bases() {
        assert!(truthy(&bases(
            r#"(def b (%mkb))
               (%bind b (lit mk) (%coord (lit str) (lit ->sym)))
               (%bind b (lit same) (%coord (lit obj) (lit eq?)))
               (match ((%beval b (lit (same (mk "+") (lit +))))
                       (%beval b (lit (same (mk "first") (lit first)))))
                      (#t ()))"#
        )));
    }

    /// And the host-read form itself runs, which is the practical consequence.
    #[test]
    fn a_form_read_in_the_host_evaluates_in_a_child() {
        assert_eq!(int_of(&bases("(%beval (%mkb) (lit (+ 2 3)))")), 5);
    }

    /// Lookup is by IDENTITY, not spelling. A child-interned symbol does not
    /// find a name the host bound under its own symbol of the same text — if it
    /// did, the per-base tables would be decoration.
    #[test]
    fn a_name_is_found_by_identity_not_by_spelling() {
        assert!(raises(&bases(
            r#"(def b (%mkb))
               (%bind b (lit answer) 42)
               (%bind b (lit mk) (%coord (lit str) (lit ->sym)))
               (%beval b (lit (eval! (mk "answer"))))"#
        )));
    }

    /// Two children intern independently of each other, not just of the host.
    #[test]
    fn two_children_do_not_share_a_table() {
        assert!(truthy(&bases(
            r#"(def mk (%coord (lit str) (lit ->sym)))
               (def b1 (%mkb)) (def b2 (%mkb))
               (%bind b1 (lit mk) mk)
               (%bind b2 (lit mk) mk)
               (match ((eq? (%beval b1 (lit (mk "shared?")))
                            (%beval b2 (lit (mk "shared?")))) ()) (#t 1))"#
        )));
    }

    /// A base is a VALUE, and its environment is recovered by walking the route
    /// base-paths.x commits to — not from a side table, so the descriptor and
    /// the engine cannot disagree about where a base keeps its bindings.
    #[test]
    fn two_bases_are_distinct_objects() {
        assert!(truthy(&bases("(match ((same? (%mkb) (%mkb)) ()) (#t 1))")));
    }
}
