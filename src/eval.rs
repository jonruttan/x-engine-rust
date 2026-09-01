//! The evaluator: the Engine, the ways a combiner can be applied, and the
//! argument boundary every primitive crosses exactly once.
//!
//! What is NOT here is the primitives themselves. They live in `src/prims/`, one
//! module per capability group in `tools/contract/isa.x`, because that partition
//! is the one the language already draws and inventing a second one would mean
//! two answers to "what does this engine implement".
//!
//! FEXPR FROM THE START. x-lang's model is operative-by-default at this level:
//! a primitive receives its arguments unevaluated and decides. Applicative
//! semantics are a wrapper over that, not the other way round, and the
//! conformance suite tests the difference with an unbound symbol — an engine
//! that evaluated an operative's arguments dies rather than merely differing.
//! The `uniform_*` row macros are a CONVENIENCE over that, not a second
//! evaluation model: a row that wants values calls `eargs` for itself, as
//! every reference primitive calls `x_eargs`.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::obj::{EnvId, Obj, NIL};

pub type EvalResult = Result<Obj, Cond>;

/// One resumable step of the evaluation in flight — what REMAINS at a
/// recursion site the engine can describe. The reference re-enters a
/// continuation by copying its C stack back; this engine cannot copy its
/// stack soundly, so `call/cc` snapshots these records instead and a
/// DEAD-extent invocation replays them, inner to outer. A capture that
/// crosses a frame with no record (an operative's private evaluation)
/// is refused at invoke time, catchably — conservative, never silently
/// wrong.
#[derive(Clone)]
pub(crate) enum ControlRec {
    /// A body mid-walk: the forms still to run after the current one.
    Body { rest: Obj, env: EnvId, depth: u32 },
    /// An argument list mid-evaluation: values so far, forms to come, and
    /// the callee to apply when they are all in.
    Args {
        callee: Obj,
        done: Vec<Obj>,
        rest: Obj,
        env: EnvId,
        n: usize,
        depth: u32,
    },
    /// A `def`/`set!` waiting on its value.
    Bind {
        name: Obj,
        env: EnvId,
        set: bool,
        depth: u32,
    },
    /// A frame whose only remaining work is to hand the value outward —
    /// `(eval form env)`'s restore has already run by the time an unwind
    /// passes it, so replay is the identity. It exists for COVERAGE: the
    /// frame is described, not skipped.
    Pass { depth: u32 },
}

impl ControlRec {
    pub(crate) fn depth(&self) -> u32 {
        match self {
            ControlRec::Body { depth, .. }
            | ControlRec::Args { depth, .. }
            | ControlRec::Bind { depth, .. }
            | ControlRec::Pass { depth } => *depth,
        }
    }
}

/// What a `call/cc` capture keeps: the control records at capture time and
/// whether they covered every in-flight frame.
pub(crate) struct ContSnapshot {
    pub k: Obj,
    pub recs: Vec<ControlRec>,
    pub resumable: bool,
}

impl Engine {
    /// GENERIC-OPERATOR DISPATCH — `x_type_op_try`, the ops half of the type
    /// system's hot path.
    ///
    /// Every value carries a type tag, ints included, so "is it typed" is not
    /// the test; CARRYING A HANDLER is. If either operand's type registers a
    /// handler for `op` in its ops alist, that handler is called as
    /// `(handler a b)` and owns any coercion. This is how the numeric tower
    /// reaches the machine operators without wrapping their names: types
    /// REGISTER ops, nothing wraps ambient `+`.
    ///
    /// Without it `(+ 2.0 2.0)` adds the two floats' operand words and answers
    /// a machine integer.
    ///
    /// When BOTH sides carry a handler, the tie is broken by the conversions the
    /// types already declare rather than by an ordering invented here: the side
    /// whose type declares a conversion FROM the other absorbs it (complex
    /// declares from float, so complex wins). Neither declaring the other:
    /// `=` answers #f — unrelated values are not equal, a question with an
    /// answer — and every other op RAISES the teaching error, since both
    /// sides registered it and the callers' raw integer path read instance
    /// payload words as integers — the address garbage the cross-engine
    /// fuzzer caught as a divergence (x-lang#584).
    ///
    /// Ops-less types have a nil ops alist, so int/int arithmetic costs a couple
    /// of slot reads and falls through.
    pub(crate) fn op_try(&mut self, op: &str, a: Obj, b: Obj) -> Result<Option<Obj>, Cond> {
        let ta = self.objects.obj_type(a);
        let tb = self.objects.obj_type(b);
        let ops_a = if ta.is_nil() {
            NIL
        } else {
            crate::prims::tok::handler(self, ta, crate::vocabulary::Family::Ops)
        };
        let ops_b = if tb.is_nil() {
            NIL
        } else {
            crate::prims::tok::handler(self, tb, crate::vocabulary::Family::Ops)
        };
        if ops_a.is_nil() && ops_b.is_nil() {
            return Ok(None);
        }
        // Interned, so the alist walk compares by pointer as the reference's does.
        let sym = self.objects.sym(op);
        let ha = self.ops_lookup(ops_a, sym);
        let hb = self.ops_lookup(ops_b, sym);
        let handler = match (ha, hb) {
            (None, None) => return Ok(None),
            (Some(h), None) => h,
            (None, Some(h)) => h,
            (Some(h), Some(_)) if ta == tb => h,
            (Some(h), Some(other)) => {
                let name_b = self.objects.handle_of_type(tb);
                let name_a = self.objects.handle_of_type(ta);
                if self.declares_from(ta, name_b) {
                    h
                } else if self.declares_from(tb, name_a) {
                    other
                } else if op == "=" {
                    // Unrelated values are not equal — a question with an
                    // answer, unlike ordering and arithmetic below.
                    return Ok(Some(self.objects.false_obj()));
                } else {
                    // Both registered the op: raise, as the reference does
                    // (x-lang#584) — byte-identical text, second type named.
                    let b_name = self.objects.sym_name(name_b);
                    let msg = crate::vocabulary::MSG_NO_CVT_RELATION.replace("{}", &b_name);
                    let v = self.objects.str_new(&msg);
                    return Err(Cond::Raised(v));
                }
            }
        };
        let env = self.root_env();
        Ok(Some(self.call_with_values(handler, &[a, b], env)?))
    }

    /// The handler `sym` names in an ops alist, compared by pointer.
    fn ops_lookup(&mut self, ops: Obj, sym: Obj) -> Option<Obj> {
        if ops.is_nil() {
            return None;
        }
        for entry in self.objects.list(ops).collect::<Vec<_>>() {
            if self.objects.first(entry) == sym {
                return Some(self.objects.rest(entry));
            }
        }
        None
    }

    /// Does `ty` declare a conversion FROM the type `name` handles?
    ///
    /// The from-alist's entries are `(type-handle . handler)` and the handle IS
    /// the type's name atom, so this compares pointers.
    fn declares_from(&mut self, ty: Obj, name: Obj) -> bool {
        let from = crate::prims::tok::handler(self, ty, crate::vocabulary::Family::From);
        if from.is_nil() {
            return false;
        }
        for entry in self.objects.list(from).collect::<Vec<_>>() {
            if self.objects.first(entry) == name {
                return true;
            }
        }
        false
    }

    // --- evaluation ---------------------------------------------------------

    /// Evaluate, WITHOUT growing the stack in tail position.
    ///
    /// x-lang requires proper tail calls. This engine recursed on the Rust
    /// stack and died somewhere between five and ten thousand frames, which is
    /// not an efficiency question: the conformance suite's own prelude burns
    /// twenty thousand iterations, so unrelated cases failed with a stack
    /// overflow — the one failure that reports nothing at all.
    ///
    /// The mechanism is the reference engine's. A form in tail position is not
    /// evaluated by recursing; it is PARKED in the base's tco rows and this loop picks
    /// it up. x-engine-c parks it in the base's `tco-expr` and `tco-env` slots
    /// for exactly the same reason.
    /// Evaluate, tracking whether anything is PENDING.
    ///
    /// The depth is not bookkeeping for its own sake: it is how this engine
    /// answers the question the reference answers with "is the save stack
    /// empty?", and the answer decides where a `def` binds. A nested `eval` is
    /// an evaluation something is waiting on — an argument, an `(eval form env)`
    /// — so a `def` inside one is local. At depth 1 nothing is waiting, the form
    /// is in tail position from the top level, and a `def` is global.
    pub fn eval(&mut self, form: Obj, env: EnvId) -> EvalResult {
        self.active_evals += 1;

        let r = self.eval_pending(form, env);
        self.active_evals -= 1;
        // THE RESULT IS ROOTED until the enclosing evaluation moves on.
        //
        // A value that has just been computed is reachable from nothing: it sits
        // in a Rust local while its caller goes on to evaluate the NEXT thing,
        // and that evaluation can collect. `apply` was caught doing exactly
        // this — holding the callee while it evaluated the argument list — and
        // every operative that evaluates more than once has the same shape.
        //
        // Rooting here rather than at each of those sites makes it a property of
        // evaluation instead of a rule to remember. The trampoline drops these
        // between steps, so a tail loop does not accumulate them.
        if let Ok(v) = r {
            self.root_push(v);
        }
        r
    }

    /// Hide the active saves for the duration of a LOAD, answering what was
    /// hidden. Each form read from a file IS a top-level form, and its `def`s
    /// bind globally however deep the load was triggered — the reference does
    /// the same to its save stack. The caller roots what it is handed: the
    /// hidden list is reachable from nothing else while the load runs.
    pub fn hide_pending(&mut self) -> Obj {
        let node = self.objects.state_nodes[crate::base::SN_SAVE];
        let held = self.objects.first(node);
        self.objects.set_data(node, 0, NIL.word());
        held
    }

    pub fn restore_pending(&mut self, outer: Obj) {
        let node = self.objects.state_nodes[crate::base::SN_SAVE];
        self.objects.set_data(node, 0, outer.word());
    }

    /// One save onto the base's save-stack row: the environment whose body (or
    /// with-env evaluation) is active. Released by [`Engine::save_pop`].
    pub(crate) fn save_push(&mut self, env: EnvId) {
        let node = self.objects.state_nodes[crate::base::SN_SAVE];
        let head = self.objects.first(node);
        let cell = self.objects.spair(env.obj(), head);
        self.objects.set_data(node, 0, cell.word());
    }

    pub(crate) fn save_pop(&mut self) {
        let node = self.objects.state_nodes[crate::base::SN_SAVE];
        let head = self.objects.first(node);
        let tail = self.objects.rest(head);
        self.objects.set_data(node, 0, tail.word());
    }

    /// The parked tail, read and cleared — the tco-expr/tco-env rows are the
    /// only place it lives.
    pub(crate) fn tail_take(&mut self) -> Option<(Obj, EnvId)> {
        let node = self.objects.state_nodes[crate::base::SN_TCO_EXPR];
        let form = self.objects.first(node);
        if form.is_nil() {
            return None;
        }
        let env_node = self.objects.state_nodes[crate::base::SN_TCO_ENV];
        let env = self.objects.first(env_node);
        self.objects.set_data(node, 0, NIL.word());
        self.objects.set_data(env_node, 0, NIL.word());
        Some((form, crate::obj::EnvId::from_obj(env)))
    }

    /// A guard is active while the base's error-handler row is non-nil.
    pub(crate) fn handler_active(&self) -> bool {
        !self
            .objects
            .first(self.objects.state_nodes[crate::base::SN_HANDLER])
            .is_nil()
    }

    pub(crate) fn handler_push(&mut self, env: EnvId) {
        let node = self.objects.state_nodes[crate::base::SN_HANDLER];
        let head = self.objects.first(node);
        let cell = self.objects.spair(env.obj(), head);
        self.objects.set_data(node, 0, cell.word());
    }

    pub(crate) fn handler_pop(&mut self) {
        let node = self.objects.state_nodes[crate::base::SN_HANDLER];
        let head = self.objects.first(node);
        let tail = self.objects.rest(head);
        self.objects.set_data(node, 0, tail.word());
    }

    /// True when the save stack is empty, which is the reference's `def`
    /// question: no closure body and no with-env evaluation is active, so the
    /// form is in tail position from the top level and a definition persists.
    /// Depth is NOT the question — `do` and every operative evaluate their
    /// forms without saving, and a `def` inside them at the top level is
    /// global (core/sandbox relies on it: one form's `(do … (def %buf-tok …))`
    /// serves the next form's tests).
    pub fn nothing_pending(&self) -> bool {
        self.objects
            .first(self.objects.state_nodes[crate::base::SN_SAVE])
            .is_nil()
    }

    fn eval_pending(&mut self, form: Obj, env: EnvId) -> EvalResult {
        let (mut form, mut env) = (form, env);
        // ONE root slot for the form under evaluation, replaced as the
        // trampoline moves rather than pushed per iteration — a long tail chain
        // would otherwise grow the root set without bound. The form came from
        // the reader and nothing else points at it, so a collection underneath
        // this call would free the code that is running.
        let mark = self.root_mark();
        self.root_push(form);
        let slot = mark;
        // The same for the ENVIRONMENT, and for the same reason: an activation
        // frame is named by this Rust local and by nothing on the heap until a
        // closure captures it, which most frames never do.
        let env_mark = self.env_root_mark();
        self.env_root_push(env);
        let env_slot = env_mark;
        let out = loop {
            // The armed ceiling. Collection is explicit-only, so between two
            // `(heap collect)` calls nothing bounds a runaway loop but this.
            // Publish an interrupt the handler recorded. Between forms is soon
            // enough and is the only safe place: the handler runs at an
            // arbitrary instruction and may not touch the heap.
            if crate::foreign::interrupted() {
                let flag = self.sigint_flag;
                self.objects.set_data(flag, 0, crate::obj::Word(1));
            }
            // A set flag becomes a STOP only while a guard can catch it: the
            // flag is cleared first so a handler that returns does not re-trip,
            // and an uncatchable raise would end the run rather than interrupt
            // the computation.
            if self.handler_active() {
                let flag = self.sigint_flag;
                if self.objects.as_int(flag) != 0 {
                    self.objects.set_data(flag, 0, crate::obj::Word(0));
                    let v = self.objects.str_new(crate::vocabulary::MSG_STOP);
                    break Err(Cond::Raised(v));
                }
            }
            // STRESS: collect far more often than x-lang ever would, to shake
            // out a root nobody remembered. A missing root frees something live,
            // and under normal use that might not surface for hours — here it
            // surfaces on the next form. Off unless asked for.
            if self.gc_stress != 0 {
                self.stress_countdown = self.stress_countdown.saturating_sub(1);
                if self.stress_countdown == 0 {
                    self.stress_countdown = self.gc_stress;
                    self.collect();
                }
            }
            if let Some(limit) = self.alloc_limit {
                if self.objects.alloc_count() > limit {
                    break Err(Cond::AllocLimit);
                }
            }
            if form.is_nil() {
                break Ok(NIL);
            }
            // The live line/file counters follow the form being evaluated —
            // the reference updates them from each expression's meta, so a
            // raise anywhere below snapshots the raise site.
            if self.objects.meta_stamped(form) {
                let line = self.objects.meta_i(form, 0);
                self.objects.set_live_line(line);
                self.live_file = self.objects.meta_i(form, 1);
            }
            // THE MACHINE, as `x_eval` draws it: a value whose type word is
            // nil or a raw marker is ITSELF, and everything else is decided by
            // its type's EVAL handler — symbol lookup is the SYMBOL type's
            // registered behaviour, application is the LIST type's, and a
            // value whose type registers nothing is itself. What evaluation
            // MEANS is data on the base, replaceable per type, per base.
            let ty = self.objects.type_of_word(form);
            if ty.is_nil() || ty == self.objects.spair_marker || ty == self.objects.satom_marker {
                break Ok(form);
            }
            let hook = self
                .objects
                .type_handler(ty, crate::vocabulary::Family::Eval);
            if hook.is_nil() {
                break Ok(form);
            }
            // An engine handler is operative-shaped and takes the form raw; a
            // LIBRARY handler — logo registers one on its block type — is a
            // closure applied to the VALUE with no argument evaluation, the
            // reference's raw-args call. The quoting door would resolve `lit`
            // through the very handler being called.
            let r = if self.objects.is_prim(hook) {
                let idx = self.objects.prim_idx(hook);
                match self.prims.get(idx).copied() {
                    Some(def) => (def.f)(self, hook, form, env),
                    None => Ok(form),
                }
            } else if self.objects.is_closure(hook) {
                // THE SETTLED ENVIRONMENT CONVENTION: the current environment
                // is an argument, as the base is. The reference's handlers read
                // it from the base's env field — its spelling under the
                // one-context-pointer constraint — so a library handler here
                // RECEIVES it, as this engine's declared door: the value
                // first, the environment second. A one-parameter handler (logo's
                // block eval) never sees the extra argument.
                let env_obj = self.objects.env_obj(env);
                self.apply_closure_values(hook, &[form, env_obj])
            } else {
                self.call_with_values(hook, &[form], env)
            };
            let answer = match r {
                Ok(v) => v,
                Err(e) => break Err(e),
            };
            // Something in tail position asked to be evaluated HERE rather than
            // under another frame. Loop instead of recursing.
            match self.tail_take() {
                Some((f, e)) => {
                    form = f;
                    env = e;
                    // A new trampoline step: the previous step's intermediate
                    // results are done with, and a long tail loop must not
                    // accumulate one root per iteration.
                    self.root_truncate(slot + 1);
                    self.roots[slot] = form;
                    self.env_root_truncate(env_slot + 1);
                    self.env_roots[env_slot] = env;
                }
                None => break Ok(answer),
            }
        };
        self.root_truncate(mark);
        self.env_root_truncate(env_mark);
        out
    }

    /// Park a form to be evaluated in the caller's own loop.
    ///
    /// Every tail position goes through here: a closure's last body form, the
    /// winning arm of a `match`, a `guard`'s handler. Anything that evaluates
    /// its tail directly would reintroduce the frame this removes.
    pub fn park_tail(&mut self, form: Obj, env: EnvId) -> Obj {
        let node = self.objects.state_nodes[crate::base::SN_TCO_EXPR];
        self.objects.set_data(node, 0, form.word());
        let env_node = self.objects.state_nodes[crate::base::SN_TCO_ENV];
        self.objects.set_data(env_node, 0, env.obj().word());
        NIL
    }

    /// Run something that may park a tail, and settle it here.
    ///
    /// Callers that are NOT a tail position — `apply`, the iterator driver, the
    /// binary's top level — need a value, not a parked form. Forgetting this is
    /// how a parked tail leaks into an unrelated evaluation.
    pub(crate) fn settle(&mut self, r: EvalResult, env: EnvId) -> EvalResult {
        let v = r?;
        match self.tail_take() {
            Some((f, e)) => self.eval(f, e),
            None => {
                let _ = env;
                Ok(v)
            }
        }
    }

    /// Apply a callee to an UNEVALUATED argument spine. `None` when the callee is
    /// not a combiner at all — the caller decides whether that is data or an
    /// error, because those two answers differ by context.
    /// A CALLABLE IS A VALUE WHOSE TYPE REGISTERS CALL — `x_type_list_eval`'s
    /// rule, and the whole of it: the operator's type names its call
    /// handler, procedure and operative and primitive included, and a type with
    /// no handler makes the form data. This is also how x-lang's class layer is
    /// reached: `lib/x/type/class.x` installs `%class-call-handler` on a
    /// class's type, and the engine finds it exactly as it finds its own.
    pub(crate) fn combine(&mut self, callee: Obj, args: Obj, env: EnvId) -> Option<EvalResult> {
        // A FOREIGN callable — `obj make-callable` over an emitted function's
        // address — applies with the C prim convention: (base, raw argument
        // spine), both as REAL addresses; the code evaluates its own
        // arguments through the jit_* door. The reference applies its
        // made-callables the same way.
        if self.objects.is_foreign(callee) {
            return Some(self.apply_foreign(callee, args, env));
        }
        let ty = self.objects.obj_type(callee);
        if ty.is_nil() {
            return None;
        }
        let hook = self
            .objects
            .type_handler(ty, crate::vocabulary::Family::Call);
        if hook.is_nil() {
            return None;
        }
        if self.objects.is_prim(hook) {
            let idx = self.objects.prim_idx(hook);
            if let Some(def) = self.prims.get(idx).copied() {
                return Some((def.f)(self, callee, args, env));
            }
        }
        // A LIBRARY handler is an OPERATIVE taking `(obj . args)`: the
        // arguments stay unevaluated and the SUBJECT goes first — the
        // selector and the rest are the handler's to interpret.
        let spine = self.objects.pair(callee, args);
        Some(self.eval_call(hook, spine, env))
    }

    /// Apply an emitted function: hand it the base and the raw argument
    /// spine as real addresses, take an object back the same way.
    fn apply_foreign(&mut self, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
        let addr = self.objects.foreign_addr(callee);
        let f = x_engine_foreign::Foreign(addr);
        let base_real = if self.base.is_nil() {
            0
        } else {
            self.objects.heap.address_of(self.base.addr())
        };
        // The C prim shape: the spine's head is SELF, the arguments follow —
        // the emitted prologue skips one cell before its first argument.
        let spine = self.objects.pair(callee, args);
        let args_real = self.objects.heap.address_of(spine.addr());
        // The code evaluates its arguments through jit_eval_arg, which must
        // resolve them where the CALL stands — the reference's x_eval_arg
        // reads the base's live environment, and a closure passing its
        // locals to an emitted function is the ordinary case.
        let prev_env = self.jit_env.replace(env);
        let out = x_engine_foreign::call_ints(f, &[base_real, args_real]);
        self.jit_env = prev_env;
        if out == 0 {
            return Ok(NIL);
        }
        match self.objects.heap.from_real(out) {
            Some(at) => Ok(at.as_obj()),
            None => Ok(NIL),
        }
    }

    /// Applying a wrapper: evaluate the arguments, then hand the VALUES to the
    /// inner operative quoted, so it does not evaluate them a second time. That
    /// is the whole of what "applicative" means here.
    pub(crate) fn apply_wrapper(&mut self, w: Obj, args: Obj, env: EnvId) -> EvalResult {
        let inner = self.objects.wrapper_inner(w);
        let vals = self.eval_args(args, env)?;

        // The spine is the VALUES, unquoted.
        //
        // `call_with_values` wraps each in `(lit v)`, which is right for a
        // closure -- it would otherwise evaluate them a second time -- and wrong
        // here. An operative binds its spine elements DIRECTLY, so a quoted
        // value arrives as the two-element form `(lit 3)` rather than as 3, and
        // `(wrap (op (x) e x))` answers a list instead of its argument.
        //
        // Which is exactly what wrapping means: evaluate, then hand the results
        // to something that does not evaluate.
        let mut spine = NIL;
        for &v in vals.iter().rev() {
            spine = self.objects.pair(v, spine);
        }
        self.eval_call(inner, spine, env)
    }

    /// Call an already-evaluated combiner. Unlike `eval`, a non-combiner here is
    /// an error: nothing wrote this form, so there is no syntax to fall back to.
    pub fn eval_call(&mut self, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
        match self.combine(callee, args, env) {
            // NOT a tail position: a caller here wants a value.
            Some(r) => self.settle(r, env),
            // NOT an error. `eval` already answers the form unchanged when the
            // head is not callable, and x-engine-c runs `(1 2)` without
            // complaint. Answering the callee keeps the two paths consistent.
            None => Ok(callee),
        }
    }

    /// Call a combiner with values already computed, IN TAIL POSITION.
    ///
    /// The difference from [`Engine::call_with_values`] is the whole of tail-call
    /// elimination: that one `settle`s, running any parked tail to a value
    /// because its callers — reader handlers, GC hooks, the class dispatcher —
    /// want one. This lets the tail stay parked so the caller's trampoline
    /// continues it, which is what `x_prim_apply` does when the callee is a
    /// procedure: it binds the parameters and returns `x_eval_body_tco`.
    ///
    /// `apply` needs it because `let` is built on it — lib/x/core/control.x
    /// expands `(let ...)` to `(apply (eval (fn ...)) vals)` — so settling here
    /// made every `let` in a tail position grow the Rust stack. 50,000 frames of
    /// `(let ((m (- n 1))) (self m))` overflowed it.
    pub fn call_with_values_tail(&mut self, callee: Obj, vals: &[Obj], env: EnvId) -> EvalResult {
        let mark = self.root_mark();
        self.root_push(callee);
        for v in vals {
            self.root_push(*v);
        }
        let spine = self.quote_values(vals);
        self.root_push(spine);
        let out = match self.combine(callee, spine, env) {
            Some(r) => r,
            None => Ok(callee),
        };
        self.root_truncate(mark);
        out
    }

    /// Call a combiner with values already computed.
    pub fn call_with_values(&mut self, callee: Obj, vals: &[Obj], env: EnvId) -> EvalResult {
        // ROOTED while the spine is BUILT. `quote_values` allocates a cell per
        // value, so a collection partway through would be free to take the
        // values still waiting in the caller's slice — and callers hand this a
        // plain Rust slice from anywhere: `apply`, the class dispatcher, every
        // reader handler.
        let mark = self.root_mark();
        self.root_push(callee);
        for v in vals {
            self.root_push(*v);
        }
        let spine = self.quote_values(vals);
        self.root_push(spine);
        let out = self.eval_call(callee, spine, env);
        self.root_truncate(mark);
        out
    }

    /// THE ARGUMENT BOUNDARY. Arity is checked and arguments are evaluated here,
    /// once, for every applicative in the engine.
    /// Evaluate an argument spine into VALUES — `x_eargs`, as every uniform
    /// row calls it for itself. The spine is rooted for the duration, and the
    /// result is padded to `n` with nil: a body indexing a slot needs the
    /// slot to exist, but a missing operand is nil, not an error — counting
    /// arguments is x-lang's job, one layer up.
    pub fn eargs(&mut self, args: Obj, env: EnvId, n: usize) -> Result<Vec<Obj>, Cond> {
        self.eargs_for(NIL, args, env, n)
    }

    /// As `eargs`, with the callee threaded through for the capture trail.
    pub fn eargs_for(
        &mut self,
        callee: Obj,
        args: Obj,
        env: EnvId,
        n: usize,
    ) -> Result<Vec<Obj>, Cond> {
        let mark = self.root_mark();
        self.root_push(args);
        let out = self.eval_args_for(callee, args, env, n);
        self.root_truncate(mark);
        let mut vals = out?;
        if vals.len() < n {
            vals.resize(n, NIL);
        }
        Ok(vals)
    }

    /// The #239 raise: "<op>: operand is nil", as the reference words it.
    pub fn nil_operand(&mut self, name: &str) -> Cond {
        let v = self.objects.str_new(&format!("{}: operand is nil", name));
        Cond::Raised(v)
    }

    /// Evaluate an argument spine into values.
    /// Evaluate an argument spine into values, ROOTING as it goes.
    ///
    /// This is the choke point every applicative passes through, and it holds
    /// two things the heap cannot see. The unevaluated FORMS are collected into
    /// a Rust Vec first — the iterator's borrow has to end before `eval` can
    /// take the objects mutably — so from that moment the spine is not what is
    /// being read; the Vec is. And each VALUE already computed exists only in
    /// the results Vec until the call is made.
    ///
    /// Evaluating argument one can collect, and did: the poison trap caught
    /// argument two being read after it was freed. Rooting here covers every
    /// caller, where rooting at each of them would have covered the ones I
    /// thought of.
    pub(crate) fn eval_args(&mut self, args: Obj, env: EnvId) -> Result<Vec<Obj>, Cond> {
        self.eval_args_for(NIL, args, env, 0)
    }

    /// As `eval_args`, leaving an [`ControlRec::Args`] trail when the callee is
    /// known — what lets a continuation captured inside an argument position
    /// replay the application.
    pub(crate) fn eval_args_for(
        &mut self,
        callee: Obj,
        args: Obj,
        env: EnvId,
        n: usize,
    ) -> Result<Vec<Obj>, Cond> {
        // An APPLICATIVE's argument list must be proper (#69): the walk
        // guards the spine and a dotted tail raises, catchably, naming the
        // fault. Ops never come through here — they receive spines raw.
        let mut at = args;
        while self.objects.is_cell(at) {
            at = self.objects.rest(at);
        }
        if !at.is_nil() {
            let v = self.objects.str_new(crate::vocabulary::MSG_IMPROPER_ARGS);
            return Err(Cond::Raised(v));
        }
        // Collected first so the iterator's borrow of the objects ends before
        // `eval` needs it mutably.
        let forms: Vec<Obj> = self.objects.list(args).collect();
        let mark = self.root_mark();
        for f in &forms {
            self.root_push(*f);
        }
        let mut out = Vec::with_capacity(forms.len());
        let mut node = args;
        for f in forms {
            node = self.objects.rest(node);
            if !callee.is_nil() {
                let depth = self.active_evals;
                self.control.push(ControlRec::Args {
                    callee,
                    done: out.clone(),
                    rest: node,
                    env,
                    n,
                    depth,
                });
            }
            let r = self.eval(f, env);
            if !callee.is_nil() {
                self.control.pop();
            }
            match r {
                Ok(v) => {
                    self.root_push(v);
                    out.push(v);
                }
                Err(c) => {
                    self.root_truncate(mark);
                    return Err(c);
                }
            }
        }
        // The FORMS are done with; the VALUES are not. They are about to be
        // bound into a frame or handed to a primitive, and `bind_params` builds
        // a rest list — an allocation, with the values reachable from nothing
        // but the caller's Vec. So the forms come off and the values go back on,
        // and they stay until the enclosing `eval_pending` truncates on exit.
        self.root_truncate(mark);
        for v in &out {
            self.root_push(*v);
        }
        Ok(out)
    }

    /// Apply a closure to VALUES, with no argument evaluation — the reference's
    /// `x_callable_call` shape, where args arrive raw. The quote-and-re-evaluate
    /// door (`call_with_values`) cannot serve a handler that redefines what a
    /// SYMBOL means: evaluating its quoted arguments resolves `lit` through the
    /// handler being called, which recurses without end.
    pub(crate) fn apply_closure_values(&mut self, callee: Obj, vals: &[Obj]) -> EvalResult {
        // ROOTED: the values arrive in a Rust slice, and binding them
        // allocates cells.
        let mark = self.root_mark();
        self.root_push(callee);
        for v in vals {
            self.root_push(*v);
        }
        let r = self.apply_closure_bound(callee, vals.to_vec());
        let out = self.settle_tail(r);
        self.root_truncate(mark);
        out
    }

    /// Settle a parked tail for a caller that needs a VALUE.
    fn settle_tail(&mut self, r: EvalResult) -> EvalResult {
        let v = r?;
        match self.tail_take() {
            Some((f, e)) => self.eval(f, e),
            None => Ok(v),
        }
    }

    /// Apply a closure. APPLICATIVE, and the first parameter is bound to the
    /// CLOSURE ITSELF — x-lang's self-passing convention, which is why every
    /// function in the conformance prelude is written with a leading `self`. It
    /// recurses without ever having been named.
    pub(crate) fn apply_closure(&mut self, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
        let vals = self.eval_args_for(callee, args, env, 0)?;
        self.apply_closure_bound(callee, vals)
    }

    /// The shared tail of both application doors: frame, params, save, body.
    fn apply_closure_bound(&mut self, callee: Obj, vals: Vec<Obj>) -> EvalResult {
        let params = self.objects.closure_params(callee);
        let body = self.objects.closure_body(callee);
        let defenv = self.objects.closure_env(callee);

        // Lexical: the new frame hangs off the DEFINING environment. A closure
        // resolving names in the CALLER's environment would be dynamic scope
        // wearing this syntax.
        // The FIRST parameter is bound to the closure itself and the rest take
        // the arguments in order, so the values line up one position behind the
        // names.
        let bound: Vec<Obj> = std::iter::once(callee).chain(vals).collect();
        let frame = self.envs.push(&mut self.objects, defenv);
        // Rooted from the moment it exists: binding a dotted rest parameter
        // allocates, and the body's non-tail forms run under it.
        let env_mark = self.env_root_mark();
        self.env_root_push(frame);
        self.bind_params(frame, params, &bound);
        // THE SAVE, as `x_tco_compound_save` draws its lifetime: held over the
        // body's non-tail forms — a `def` there is the activation's own — and
        // released before the parked tail runs, which is why a tail `def`
        // binds globally. Operatives do not save; the reference's own answer
        // to a def reaching through an op's tail-eval says so.
        self.save_push(frame);
        let out = self.eval_body_tail(body, frame);
        self.save_pop();
        self.env_root_truncate(env_mark);
        out
    }

    /// Apply an operative: arguments arrive UNEVALUATED and the caller's
    /// environment is handed over as a value. No self-binding — the two kinds
    /// differ in what they are for.
    ///
    /// The body runs off the OPERATIVE's own environment, so its scope is lexical
    /// like everything else; the caller's environment is reachable only through
    /// the name the operative asked for, which makes reaching into it deliberate.
    pub(crate) fn apply_op(&mut self, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
        let params = self.objects.op_params(callee);
        let envname = self.objects.op_envname(callee);
        let body = self.objects.op_body(callee);
        let defenv = self.objects.op_env(callee);

        // Arguments arrive AS WRITTEN and the spine binds STRUCTURALLY: a
        // rest parameter takes the remaining spine as it stands, so a dotted
        // param spec binds an atom tail legitimately (#69).
        let frame = self.envs.push(&mut self.objects, defenv);
        let env_mark = self.env_root_mark();
        self.env_root_push(frame);
        let mut p = params;
        let mut a = args;
        while self.objects.is_cell(p) {
            let name = self.objects.first(p);
            let v = if self.objects.is_cell(a) {
                let v = self.objects.first(a);
                a = self.objects.rest(a);
                v
            } else {
                let v = a;
                a = NIL;
                v
            };
            self.envs.bind(&mut self.objects, frame, name, v);
            p = self.objects.rest(p);
        }
        if !p.is_nil() {
            self.envs.bind(&mut self.objects, frame, p, a);
        }
        if !envname.is_nil() {
            let e = self.objects.env_obj(env);
            self.envs.bind(&mut self.objects, frame, envname, e);
        }
        let out = self.eval_body_tail(body, frame);
        self.env_root_truncate(env_mark);
        out
    }

    /// Bind a parameter list to values, honouring a DOTTED REST PARAMETER.
    ///
    /// `(fn (_ . args) ...)` names a list, not a position: the tail of the
    /// parameter spine is a symbol rather than nil, and everything not already
    /// bound goes to it as a list. x-lang's reader protocol depends on it —
    /// every `read` handler in the conformance suite is written that way — and
    /// a binder that only walked proper lists would leave the name unbound while
    /// silently accepting the definition.
    fn bind_params(&mut self, frame: EnvId, params: Obj, vals: &[Obj]) {
        let mut p = params;
        let mut i = 0usize;
        while self.objects.is_cell(p) {
            let name = self.objects.first(p);
            let v = vals.get(i).copied().unwrap_or(NIL);
            self.envs.bind(&mut self.objects, frame, name, v);
            i += 1;
            p = self.objects.rest(p);
        }
        // A non-nil tail is the rest parameter.
        if !p.is_nil() {
            let mut list = NIL;
            for &v in vals[i.min(vals.len())..].iter().rev() {
                list = self.objects.pair(v, list);
            }
            self.envs.bind(&mut self.objects, frame, p, list);
        }
    }

    /// Evaluate a sequence, answering the last value.
    /// Evaluate all but the last form, and PARK the last.
    ///
    /// The last form of a body is in tail position, so evaluating it here would
    /// be the recursion this whole mechanism exists to avoid.
    pub fn eval_body_tail(&mut self, body: Obj, env: EnvId) -> EvalResult {
        let forms: Vec<Obj> = self.objects.list(body).collect();
        let Some((last, rest)) = forms.split_last() else {
            return Ok(NIL);
        };
        let mut node = body;
        for f in rest {
            node = self.objects.rest(node);
            let depth = self.active_evals;
            self.control.push(ControlRec::Body {
                rest: node,
                env,
                depth,
            });
            let r = self.eval(*f, env);
            self.control.pop();
            r?;
        }
        Ok(self.park_tail(*last, env))
    }

    pub fn eval_body(&mut self, body: Obj, env: EnvId) -> EvalResult {
        // Collected first so the iterator's borrow of the objects ends before
        // `eval` needs it mutably.
        let forms: Vec<Obj> = self.objects.list(body).collect();
        let mut last = NIL;
        let mut node = body;
        for f in forms {
            node = self.objects.rest(node);
            let depth = self.active_evals;
            self.control.push(ControlRec::Body {
                rest: node,
                env,
                depth,
            });
            let r = self.eval(f, env);
            self.control.pop();
            last = r?;
        }
        Ok(last)
    }

    // --- helpers the primitive modules share ---------------------------------

    /// The nth element of an unevaluated spine. Operatives only: an applicative
    /// is handed a slice and has no spine to walk.
    pub fn nth(&self, mut l: Obj, n: usize) -> Obj {
        for _ in 0..n {
            if !self.objects.is_cell(l) {
                return NIL;
            }
            l = self.objects.rest(l);
        }
        if self.objects.is_cell(l) {
            self.objects.first(l)
        } else {
            NIL
        }
    }

    /// Wrap values in `(lit x)` so that calling a combiner with an
    /// already-computed argument list does not evaluate them a second time. For
    /// a symbol value the difference is a live unbound-name error, not a nuance.
    pub fn quote_values(&mut self, vals: &[Obj]) -> Obj {
        let lit = self.objects.sym(crate::vocabulary::LIT);
        let mut out = NIL;
        for &v in vals.iter().rev() {
            let inner = self.objects.pair(v, NIL);
            let q = self.objects.pair(lit, inner);
            out = self.objects.pair(q, out);
        }
        out
    }

    // --- reading operands -----------------------------------------------
    // UNCHECKED, every one. An engine is a machine: it reads the word at a
    // slot and applies an operator. Deciding that a word is "not a number" is
    // a TYPE judgement, and types are x-lang's, one layer up.
    //
    // x-lang's contract already ruled first/rest unchecked; these are the same
    // rule. They were written as checks anyway, and each check individually
    // looked like an improvement while collectively pulling the type system
    // down into the machine.
    //
    // x-engine-c agrees by demonstration: `(+ 1 (lit a))` and `(1 2)` both run
    // there. Nothing here can fail, so nothing here returns a Result.

    // --- what the process boundary needs ------------------------------------
}

#[cfg(test)]
mod tests {
    use crate::testkit::{eval_ok, int_of, raises, truthy};

    // These exercise the EVALUATOR directly, not the instructions that ride
    // on it: what self-evaluates, what a non-callable head does, how arguments
    // line up with names.

    #[test]
    fn literals_evaluate_to_themselves() {
        assert_eq!(int_of("7"), 7);
        assert!(truthy(r#"(eq? (lit ()) ())"#));
    }

    #[test]
    fn a_symbol_resolves_and_an_unbound_one_raises() {
        assert_eq!(int_of("(def x 5) x"), 5);
        assert!(raises("no-such-name"));
    }

    /// A head that is not callable makes the form DATA. This is how quoted
    /// structures survive evaluation, and x-engine-c agrees: `(1 2)` runs there.
    #[test]
    fn a_non_callable_head_makes_the_form_data() {
        assert!(!raises("(1 2)"));
        assert!(truthy("(eq? (first (1 2)) 1)"));
    }

    /// Arguments are evaluated LEFT TO RIGHT and exactly once. A second
    /// evaluation would be invisible for constants and fatal for a symbol.
    #[test]
    fn arguments_are_evaluated_once_each() {
        assert_eq!(
            int_of("(def n 0) (def bump (fn (self) (set! n (+ n 1)))) (+ (bump) (bump)) n"),
            2
        );
    }

    /// Missing operands are nil (which `+` then refuses under #52), extra
    /// ones ignored — a machine reads the slots its instruction declares.
    #[test]
    fn operands_are_padded_and_extras_dropped() {
        assert!(raises("(+ 1)"));
        assert_eq!(int_of("(+ 1 2 99)"), 3);
    }

    /// `fn` binds its first parameter to the closure itself, so the values line
    /// up one position behind the names.
    #[test]
    fn a_closures_values_line_up_behind_its_names() {
        assert_eq!(int_of("((fn (self a b) (- a b)) 9 4)"), 5);
        assert!(truthy("((fn (self) (same? self self)))"));
    }

    /// A rest parameter takes everything left over, including nothing.
    #[test]
    fn a_rest_parameter_collects_what_remains() {
        assert_eq!(int_of("((fn (self a . more) (first more)) 1 2 3)"), 2);
        assert!(truthy("(eq? ((fn (self . more) more) ) ())"));
    }

    /// The frame hangs off the DEFINING environment, not the caller's — but
    /// the probe must use a NON-tail def: a def in a closure's TAIL runs after
    /// the save is released and binds globally, exactly as the reference's
    /// x_prim_define documents ("the settled tail-def-binds-globally
    /// semantics"). Asked of x-engine-c: the tail shape answers 2 and updates
    /// the global; the non-tail shape stays the activation's own.
    #[test]
    fn a_closure_resolves_names_where_it_was_written() {
        assert_eq!(
            int_of("(def m 1) (def get (fn (self) m)) (def call (fn (self) (%seq (def m 5) ()) (get))) (call)"),
            1
        );
    }

    /// A def that is (part of) a closure's parked tail binds GLOBALLY: the
    /// save covers the body's non-tail forms only. include/import and the
    /// doc-wrapping machinery rely on it.
    #[test]
    fn a_tail_def_binds_globally() {
        assert_eq!(
            int_of("(def n 1) (def get (fn (self) n)) (def call (fn (self) (%seq (def n 2) (get)))) (call)"),
            2
        );
        assert_eq!(
            int_of("(def n 1) (def call (fn (self) (%seq (def n 2) ()))) (call) n"),
            2
        );
    }

    /// The settled environment convention, observable: a library eval handler
    /// receives the environment of the evaluation as its second argument, so
    /// a handler that decides what its instances mean can resolve names in the
    /// scope they are evaluated in — the door a JavaScript interpreter needs
    /// open. On a MADE type: a handler replacing the SYMBOL type would resolve
    /// its own body through itself, which is the per-base stamping question,
    /// not this one.
    #[test]
    fn a_library_eval_handler_receives_the_environment() {
        let program = format!(
            "{}{}",
            crate::testkit::CATALOG,
            "(def %tmake (%coord (lit type) (lit make)))
             (def %minst (%coord (lit type) (lit make-instance)))
             (def h (%tmake \"ENV-EVAL\" (pair (pair (lit eval)
               (fn (self v env) (eval (lit target) env))) ())))
             (def i (%minst h 7))
             (def target 55)
             (eval i ())"
        );
        assert_eq!(
            crate::testkit::int_of(&program),
            55,
            "the hook resolved a name through the environment it was handed"
        );
    }

    /// THE ARC'S ACCEPTANCE: what evaluation MEANS is type data. Replace the
    /// SYMBOL type's eval handler and every symbol means something else; restore
    /// it and the old meaning returns. This is the door a JavaScript
    /// interpreter — or a CPU — walks in through.
    #[test]
    fn evaluation_is_replaceable_through_the_type() {
        let mut e = crate::engine::Engine::new();
        let hook = e.eval_str("(fn (_ s) 42)").unwrap();
        let base = e.objects.base;
        let ty = e.objects.builtin_type_in(base, crate::objects::FLAG_SYM);
        let old = e.objects.type_handler(ty, crate::vocabulary::Family::Eval);
        e.objects
            .type_set_handler(ty, crate::vocabulary::Family::Eval, hook);
        let v = e.eval_str("certainly-unbound").unwrap();
        assert_eq!(e.objects.as_int(v), 42, "the replaced meaning governs");
        e.objects
            .type_set_handler(ty, crate::vocabulary::Family::Eval, old);
        assert!(
            e.eval_str("certainly-unbound").is_err(),
            "and the restored one raises unbound again"
        );
    }

    /// E2's acceptance, the twin of E1's: what APPLICATION means is type
    /// data. Replace the PROCEDURE type's call handler and calling any closure
    /// means something else; restore it and application returns.
    #[test]
    fn application_is_replaceable_through_the_type() {
        let mut e = crate::engine::Engine::new();
        let hook = e.eval_str("(op (f . a) env 99)").unwrap();
        let base = e.objects.base;
        let ty = e.objects.builtin_type_in(base, crate::objects::FLAG_FN);
        let old = e.objects.type_handler(ty, crate::vocabulary::Family::Call);
        e.objects
            .type_set_handler(ty, crate::vocabulary::Family::Call, hook);
        let v = e.eval_str("((fn (self x) x) 7)").unwrap();
        assert_eq!(
            e.objects.as_int(v),
            99,
            "the replaced meaning governs every closure call"
        );
        e.objects
            .type_set_handler(ty, crate::vocabulary::Family::Call, old);
        let v = e.eval_str("((fn (self x) x) 7)").unwrap();
        assert_eq!(e.objects.as_int(v), 7, "and the restored one applies again");
    }

    /// An operative gets its spine AS WRITTEN, so an unbound name survives.
    #[test]
    fn an_operative_receives_forms_not_values() {
        assert!(truthy("(def q (op (x) e x)) (eq? (q nope) (lit nope))"));
    }

    /// Nested calls unwind a raise all the way out rather than swallowing it.
    #[test]
    fn a_raise_propagates_through_nested_calls() {
        assert!(raises("((fn (self) ((fn (s2) (error 1)))))"));
        assert_eq!(int_of("(guard (e 9) ((fn (self) (error 1))))"), 9);
    }

    /// `eval_str` answers the LAST form's value, which is what makes the
    /// embedding API usable and what every test here depends on.
    #[test]
    fn a_source_string_answers_its_last_form() {
        let (e, v) = eval_ok("(def a 1) (def b 2) (+ a b)");
        assert_eq!(e.objects.as_int(v), 3);
    }
}
