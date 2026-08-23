//! The evaluator's own doors: eval, apply, wrap, and the reflective root.
//!
//! Mirrors the reference engine's `x-prim/core.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj};
use crate::objects::Objects;
use crate::prim::PrimDef;

/// `(eval expr env)` — in the environment given, which is how an operative
/// reaches into its caller's.
fn eval_in(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let expr_form = e.nth(args, 0);
    let expr = e.eval(expr_form, env)?;
    let env_form = e.nth(args, 1);
    let target_obj = e.eval(env_form, env)?;
    let target = if e.objects.is_env(target_obj) {
        e.objects.env_id(target_obj)
    } else {
        env
    };
    e.eval(expr, target)
}

/// `(eval! expr)` — in the CURRENT environment. The REPL's door, and what lets a
/// name held in a variable be resolved.
fn eval_here(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let form = e.nth(args, 0);
    let expr = e.eval(form, env)?;
    e.eval(expr, env)
}

/// `(apply f args)` — call with an argument list already built. The elements are
/// VALUES, not expressions: passing them unquoted would evaluate them a second
/// time, and for a symbol value that is a live unbound-name error.
fn apply(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let f_form = e.nth(args, 0);
    let f = e.eval(f_form, env)?;
    let l_form = e.nth(args, 1);
    let list = e.eval(l_form, env)?;
    let vals: Vec<Obj> = e.objects.list(list).collect();
    e.call_with_values(f, &vals, env)
}

/// The reflective root. Everything reflective starts here: the prelude walks the
/// committed base paths from `(%base)` to reach the prims catalog, so an engine
/// without it cannot even be asked what it provides.
fn base(e: &mut Engine, _a: &[Obj]) -> EvalResult {
    Ok(e.base)
}

/// `(wrap o)` — an applicative over an operative.
///
/// Holds the operative ITSELF, so `unwrap` can answer the very same object.
/// Rebuilding an equivalent one would pass every behavioural test and fail
/// `same?`, which is what the library relies on when it strips and re-wraps a
/// combiner without losing its identity.
fn wrap(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.wrapper(a[0]))
}

/// `(unwrap w)` — the operative back out, unchecked like every other operand
/// read: the data word is the answer.
fn unwrap(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.data(a[0], 0).as_obj())
}

/// `(atomic body...)` — its body's value.
///
/// A sequencing point. There is nothing to make atomic in a single-threaded
/// engine with no collector to interleave with, so it is its body and no more,
/// which is exactly what x-lang asserts of it.
fn atomic(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    e.eval_body_tail(args, env)
}

/// `(tail-eval expr env)` — the operative's door back into evaluation.
fn tail_eval(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let target = if e.objects.is_env(a[1]) {
        e.objects.env_id(a[1])
    } else {
        e.root_env()
    };
    e.eval(a[0], target)
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::op("eval", eval_in),
    PrimDef::op("eval!", eval_here),
    PrimDef::op("apply", apply),
    PrimDef::op("atomic", atomic),
    PrimDef::bare("wrap", 1, wrap),
    PrimDef::bare("unwrap", 1, unwrap),
    PrimDef::bare_full("tail-eval", 2, tail_eval),
    PrimDef::bare_full("%base", 0, base),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{int_of, truthy};

    /// Evaluate, then hand the RESULTS to something that does not evaluate.
    #[test]
    fn wrap_makes_an_operative_applicative() {
        assert_eq!(
            int_of("(def o (op (x) e x)) (def w (wrap o)) (w (+ 1 2))"),
            3
        );
    }

    /// Identity, not equivalence. An implementation that rebuilt an equal
    /// operative would pass every behavioural test and fail this one, and the
    /// library relies on it to strip and re-wrap a combiner without losing
    /// its identity.
    #[test]
    fn unwrap_recovers_the_very_same_operative() {
        assert!(truthy("(def o (op (x) e x)) (same? (unwrap (wrap o)) o)"));
    }

    #[test]
    fn atomic_yields_its_bodys_value() {
        assert_eq!(int_of("(atomic (+ 20 22))"), 42);
    }

    #[test]
    fn tail_eval_uses_the_environment_it_is_given() {
        assert_eq!(
            int_of("(def probe (op (x) e (tail-eval x e))) (def y 40) (probe (+ y 2))"),
            42
        );
    }

    /// The elements of the list are values. Unquoted, a symbol among them would
    /// be evaluated and raise.
    #[test]
    fn apply_does_not_evaluate_the_argument_list_twice() {
        assert!(truthy(
            "(def head (fn (self x) x))
             (eq? (apply head (pair (lit no-such-name) ())) (lit no-such-name))"
        ));
    }

    #[test]
    fn base_is_not_nil() {
        assert!(truthy("(match ((eq? (%base) ()) ()) (#t 1))"));
    }
}
