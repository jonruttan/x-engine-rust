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
///
/// The two arities are DIFFERENT instructions, as `x_prim_eval` draws them.
/// Without an env the expression is in tail position — it is PARKED for the
/// caller's trampoline, so a loop written through `eval` runs in constant
/// stack. With an env it cannot be: the given environment must be restored
/// after, so the evaluation happens here, under this frame.
fn eval_in(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let expr_form = e.nth(args, 0);
    let expr = e.eval(expr_form, env)?;
    // Presence is asked of the SPINE, not the value: `(eval x ())` has an env
    // argument that happens to be nil, and takes the with-env path.
    let env_cell = e.objects.rest(args);
    if env_cell.is_nil() {
        return Ok(e.park_tail(expr, env));
    }
    let target_obj = e.eval(e.objects.first(env_cell), env)?;
    let target = if e.objects.is_env(target_obj) {
        e.objects.env_id(target_obj)
    } else {
        env
    };
    // With-env SAVES, as x_prim_eval pushes its compound save: a `def` inside
    // binds into the given environment, not globally.
    e.saves += 1;
    let out = e.eval(expr, target);
    e.saves -= 1;
    out
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
    // TAIL, not settled: `let` expands through here.
    e.call_with_values_tail(f, &vals, env)
}

/// The reflective root. Everything reflective starts here: the prelude walks the
/// committed base paths from `(%base)` to reach the prims catalog, so an engine
/// without it cannot even be asked what it provides.
fn base(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
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
/// A sequencing point. Nothing here interleaves with a body: the engine is
/// single-threaded and collection only happens where the evaluator asks for it,
/// so this is its body and no more — which is exactly what x-lang asserts of it.
fn atomic(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    e.eval_body_tail(args, env)
}

/// `(tail-eval expr env)` — the operative's door back into evaluation, IN TAIL
/// POSITION.
///
/// It PARKS the tail rather than evaluating nested, and the difference is
/// visible in x-lang rather than being an optimisation. The reference decides
/// whether a `def` binds globally or locally by asking whether its save stack is
/// empty, and a tail-eval leaves nothing on it — so a `def` reached through a
/// chain of tail-evals from the top level binds GLOBALLY, even though the env
/// handed along was some inner operative's activation frame.
///
/// Asked directly of x-engine-c, since no document rules on it:
///
/// ```text
/// (def myif (op (t th . el) e
///   (match ((eval t e) (tail-eval th e)) (#t (tail-eval (first el) e)))))
/// (def outer (op (x) e (myif #f 1 (def zz 7))))
/// (outer 0)
/// zz          =>  7
/// ```
///
/// `lib/x/doc/doc.x` is built on this exact behaviour — its comment says the
/// final tail-eval "must run in the op's own tail so it defines the symbol in
/// the caller's env".
fn tail_eval(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    // An env operand that is not an env is a caller error, not a licence to
    // pick one.
    if !e.objects.is_env(a[1]) {
        return Err(Cond::NotAnEnvironment(a[1]));
    }
    let target = e.objects.env_id(a[1]);
    Ok(e.park_tail(a[0], target))
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
