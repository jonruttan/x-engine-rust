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

/// The SYMBOL type's eval handler — `x_type_symbol_eval`: a symbol evaluates to
/// what the environment binds it to, and an unbound one raises. Registered on
/// every base's SYMBOL type; the machine itself does not know what a symbol
/// means.
pub(crate) fn sym_eval(e: &mut Engine, form: Obj, env: EnvId) -> EvalResult {
    match e.envs.lookup(&e.objects, env, form) {
        Some(v) => Ok(v),
        None => Err(crate::diag::Cond::Unbound(form)),
    }
}

/// The LIST type's eval handler — `x_type_list_eval`: evaluate the head, then
/// apply through the machinery; a head that is not callable makes the form
/// DATA, which is how quoted structures survive being evaluated. A parked
/// tail flows back to the caller's trampoline, as the reference's tco_expr
/// does.
pub(crate) fn list_eval(e: &mut Engine, form: Obj, env: EnvId) -> EvalResult {
    let head = e.objects.first(form);
    let args = e.objects.rest(form);
    let callee = e.eval(head, env)?;
    match e.combine(callee, args, env) {
        Some(r) => r,
        None => Ok(form),
    }
}

/// THE ONE CALL DOOR — `x_callable_call` in table-index spelling. Every
/// callable's slot 0 is its ENTRY: for an instruction, the instruction
/// itself; for a closure, operative, wrap or continuation, the entry row a
/// constructor stamped. One read, one dispatch — the callee's KIND is never
/// consulted. A word that misses the table (a foreign address — its
/// invocation ABI belongs to the jit lane, undeclared here) declines, and
/// the form stays data.
fn callable_call(e: &mut Engine, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
    let idx = e.objects.data(callee, 0).as_usize();
    match e.prims.get(idx).copied() {
        Some(def) => (def.f)(e, callee, args, env),
        // A word that misses the table — a foreign address, whose invocation
        // ABI belongs to the undeclared jit lane — is not callable. The form
        // is data; `combine`'s caller keeps that contract, so the answer here
        // is the callee, as `eval_call` answers a non-combiner.
        None => Ok(callee),
    }
}

/// The four ENTRIES. Each is the whole behaviour of applying its kind — the
/// fast paths and argument handling live INSIDE, never as dispatcher cases.
fn procedure_entry(e: &mut Engine, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
    e.apply_closure(callee, args, env)
}

fn operative_entry(e: &mut Engine, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
    e.apply_op(callee, args, env)
}

fn wrap_entry(e: &mut Engine, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
    e.apply_wrapper(callee, args, env)
}

fn cont_entry(e: &mut Engine, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
    let vals = e.eargs(args, env, 1)?;
    e.invoke_cont(callee, vals[0])
}

/// The entry table, minted at registration in `Objects::entry_words` order:
/// procedure, operative, wrap, continuation — then the shared door.
#[rustfmt::skip]
pub(crate) const CALL_ENTRIES: &[PrimDef] = &[
    PrimDef::row(Some("%procedure-entry"), None, 0, procedure_entry),
    PrimDef::row(Some("%operative-entry"), None, 0, operative_entry),
    PrimDef::row(Some("%wrap-entry"), None, 0, wrap_entry),
    PrimDef::row(Some("%cont-entry"), None, 0, cont_entry),
];

#[rustfmt::skip]
pub(crate) const CALLABLE_CALL: PrimDef =
    PrimDef::row(Some("%callable-call"), None, 0, callable_call);

/// The LIST type's call handler — the reference's `x_type_list_call`:
/// `(lst i)` indexes, a negative index counting from the end; `(lst start
/// len)` slices. Arguments evaluate; out of range answers nil.
fn list_call(e: &mut Engine, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
    if args.is_nil() {
        return Ok(crate::obj::NIL);
    }
    let two = !e.objects.rest(args).is_nil();
    let vals = e.eargs(args, env, if two { 2 } else { 1 })?;
    let mut at = callee;
    if two {
        let start = e.objects.as_int(vals[0]);
        let len = e.objects.as_int(vals[1]);
        for _ in 0..start.max(0) {
            if at.is_nil() {
                break;
            }
            at = e.objects.rest(at);
        }
        let mut items = Vec::new();
        for _ in 0..len.max(0) {
            if at.is_nil() {
                break;
            }
            items.push(e.objects.first(at));
            at = e.objects.rest(at);
        }
        let mut out = crate::obj::NIL;
        for &o in items.iter().rev() {
            out = e.objects.pair(o, out);
        }
        return Ok(out);
    }
    let mut n = e.objects.as_int(vals[0]);
    if n < 0 {
        let mut len = 0i64;
        let mut w = callee;
        while !w.is_nil() {
            len += 1;
            w = e.objects.rest(w);
        }
        n += len;
    }
    if n < 0 {
        return Ok(crate::obj::NIL);
    }
    for _ in 0..n {
        if at.is_nil() {
            break;
        }
        at = e.objects.rest(at);
    }
    Ok(if at.is_nil() {
        crate::obj::NIL
    } else {
        e.objects.first(at)
    })
}

#[rustfmt::skip]
pub(crate) const LIST_CALL: PrimDef =
    PrimDef::row(Some("%list-call"), None, 0, list_call);

/// The handler table, minted at registration: symbol, then list. Operative-shaped
/// — a handler receives the FORM raw and the environment, which is the engine
/// dispatch's own hand-off.
#[rustfmt::skip]
pub(crate) const EVAL_HANDLERS: &[PrimDef] = &[
    PrimDef::row(Some("%sym-eval"), None, 0, sym_eval_u),
    PrimDef::row(Some("%list-eval"), None, 0, list_eval_u),
];

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
    e.save_push(target);
    let depth = e.active_evals;
    e.control.push(crate::eval::ControlRec::Pass { depth });
    let out = e.eval(expr, target);
    e.control.pop();
    e.save_pop();
    out
}

/// `(eval! expr)` — in the CURRENT environment. The REPL's door, and what lets a
/// name held in a variable be resolved.
fn eval_here(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let form = e.nth(args, 0);
    let expr = e.eval(form, env)?;
    let depth = e.active_evals;
    e.control.push(crate::eval::ControlRec::Pass { depth });
    let out = e.eval(expr, env);
    e.control.pop();
    out
}

/// `(apply f args)` — call with an argument list already built. The elements are
/// VALUES, not expressions: passing them unquoted would evaluate them a second
/// time, and for a symbol value that is a live unbound-name error.
fn apply(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let f_form = e.nth(args, 0);
    let f = e.eval(f_form, env)?;
    // Prefix arguments prepend to the LAST argument, the tail list:
    // (apply f a b (list c d)) calls f with (a b c d), as the reference
    // splices them.
    let rest = e.objects.rest(args);
    let evaled = e.eval_args(rest, env)?;
    let mut vals: Vec<Obj> = Vec::new();
    if let Some((last, prefix)) = evaled.split_last() {
        vals.extend_from_slice(prefix);
        vals.extend(e.objects.list(*last));
    }
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

/// `(unwrap w)` — the operative back out: a wrap's STATE slot holds the
/// combiner, its entry slot how to apply it.
fn unwrap(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.wrapper_inner(a[0]))
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

crate::uniform_op!(sym_eval_u, sym_eval);
crate::uniform_op!(list_eval_u, list_eval);
crate::uniform_op!(eval_in_u, eval_in);
crate::uniform_op!(eval_here_u, eval_here);
crate::uniform_op!(apply_u, apply);
crate::uniform_op!(atomic_u, atomic);
crate::uniform_value!(wrap_u, wrap, 1);
crate::uniform_value!(unwrap_u, unwrap, 1);
crate::uniform_engine!(tail_eval_u, tail_eval, 2);
crate::uniform_engine!(base_u, base, 0);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(Some("eval"), None, 0, eval_in_u),
    PrimDef::row(Some("eval!"), None, 0, eval_here_u),
    PrimDef::row(Some("apply"), None, 0, apply_u),
    PrimDef::row(Some("atomic"), None, 0, atomic_u),
    PrimDef::row(Some("wrap"), None, 1, wrap_u),
    PrimDef::row(Some("unwrap"), None, 1, unwrap_u),
    PrimDef::row(Some("tail-eval"), None, 2, tail_eval_u),
    PrimDef::row(Some("%base"), None, 0, base_u),
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
