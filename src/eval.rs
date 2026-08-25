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
//! `Body::Applicative` is therefore a CONVENIENCE the dispatcher provides, not a
//! second evaluation model: it evaluates the spine and checks arity so that a
//! hundred primitives do not each do it by hand.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::obj::{EnvId, Obj, NIL};
use crate::prim::{Body, PrimDef};

pub type EvalResult = Result<Obj, Cond>;

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
    /// declares from float, so complex wins). Neither declaring the other falls
    /// through — an unrelated pair is not this layer's to decide.
    ///
    /// Ops-less types have a nil ops alist, so int/int arithmetic costs a couple
    /// of slot reads and falls through.
    pub(crate) fn op_try(&mut self, op: &str, a: Obj, b: Obj) -> Result<Option<Obj>, Cond> {
        let ta = self.objects.type_tree_of(a);
        let tb = self.objects.type_tree_of(b);
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
                let name_b = self.objects.type_handle_of_tree(tb);
                let name_a = self.objects.type_handle_of_tree(ta);
                if self.declares_from(ta, name_b) {
                    h
                } else if self.declares_from(tb, name_a) {
                    other
                } else {
                    return Ok(None);
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
            if self.objects.first(self.objects.first(entry)) == name {
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
    /// evaluated by recursing; it is PARKED in `self.tail` and this loop picks
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
        self.eval_depth += 1;
        self.active_evals += 1;
        let r = self.eval_pending(form, env);
        self.active_evals -= 1;
        self.eval_depth -= 1;
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

    /// Hide what is pending for the duration of a LOAD, answering what was
    /// hidden. See `include`.
    pub fn hide_pending(&mut self) -> usize {
        std::mem::replace(&mut self.eval_depth, 0)
    }

    pub fn restore_pending(&mut self, outer: usize) {
        self.eval_depth = outer;
    }

    /// True when nothing is waiting on the current evaluation.
    ///
    /// `def` asks this to decide global-vs-local, exactly as the reference asks
    /// whether its save stack is empty.
    pub fn nothing_pending(&self) -> bool {
        self.eval_depth <= 1
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
            if self.guard_depth > 0 {
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
            if self.objects.is_sym(form) {
                break match self.envs.lookup(&self.objects, env, form) {
                    Some(v) => Ok(v),
                    None => Err(Cond::Unbound(form)),
                };
            }
            if !self.objects.is_cell(form) {
                // Integers, strings, closures, primitives: self-evaluating.
                break Ok(form);
            }
            let head = self.objects.first(form);
            let args = self.objects.rest(form);
            let callee = match self.eval(head, env) {
                Ok(c) => c,
                Err(e) => break Err(e),
            };
            let answer = match self.combine(callee, args, env) {
                Some(Ok(v)) => v,
                Some(Err(e)) => break Err(e),
                // A head that is not callable makes the form DATA, which is how
                // x-lang's quoted structures survive being evaluated.
                None => break Ok(form),
            };
            // Something in tail position asked to be evaluated HERE rather than
            // under another frame. Loop instead of recursing.
            match self.tail.take() {
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
        self.tail = Some((form, env));
        NIL
    }

    /// Run something that may park a tail, and settle it here.
    ///
    /// Callers that are NOT a tail position — `apply`, the iterator driver, the
    /// binary's top level — need a value, not a parked form. Forgetting this is
    /// how a parked tail leaks into an unrelated evaluation.
    fn settle(&mut self, r: EvalResult, env: EnvId) -> EvalResult {
        let v = r?;
        match self.tail.take() {
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
    fn combine(&mut self, callee: Obj, args: Obj, env: EnvId) -> Option<EvalResult> {
        if self.objects.is_prim(callee) {
            let def = self.prims[self.objects.prim_idx(callee)];
            return Some(self.call_prim(&def, args, env));
        }
        if self.objects.is_closure(callee) {
            return Some(self.apply_closure(callee, args, env));
        }
        if self.objects.is_op(callee) {
            return Some(self.apply_op(callee, args, env));
        }
        if self.objects.is_wrapper(callee) {
            return Some(self.apply_wrapper(callee, args, env));
        }
        if self.objects.is_cont(callee) {
            let v = match self.eval_args(args, env) {
                Ok(vals) => vals.first().copied().unwrap_or(NIL),
                Err(c) => return Some(Err(c)),
            };
            return Some(self.invoke_cont(callee, v));
        }
        // VALUE-CALL DISPATCH, and it is the last thing tried on purpose: a
        // value whose TYPE carries a `call` handler is callable.
        //
        // This is how x-lang's whole class layer is reached. `(Type of 1)` has a
        // CLASS at its head, not a closure — `lib/x/type/class.x` installs
        // `%class-call-handler` on the class's type, and the engine's job is to
        // find it and hand the form over. Without this the head is simply not
        // callable, the form falls through to the data rule, and `(Type of 1)`
        // evaluates to the LIST `(Type of 1)`. Nothing raises; every class call
        // in the library quietly answers its own source text.
        //
        // The handler is an OPERATIVE taking `(obj . args)`, so the arguments
        // stay unevaluated and the SUBJECT goes first — the selector and the
        // rest are the handler's to interpret, not this engine's.
        if let Some(handler) = self.call_handler_for(callee) {
            let spine = self.objects.pair(callee, args);
            return Some(self.eval_call(handler, spine, env));
        }
        None
    }

    /// The `call` handler installed on a value's type, if any.
    fn call_handler_for(&mut self, callee: Obj) -> Option<Obj> {
        // The TREE, not the handle: handlers live in the tree.
        let ty = self.objects.type_tree_of(callee);
        if ty.is_nil() {
            return None;
        }
        let h = self
            .objects
            .type_handler(ty, crate::vocabulary::Family::Call);
        if h.is_nil() {
            None
        } else {
            Some(h)
        }
    }

    /// Applying a wrapper: evaluate the arguments, then hand the VALUES to the
    /// inner operative quoted, so it does not evaluate them a second time. That
    /// is the whole of what "applicative" means here.
    fn apply_wrapper(&mut self, w: Obj, args: Obj, env: EnvId) -> EvalResult {
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
    fn call_prim(&mut self, def: &PrimDef, args: Obj, env: EnvId) -> EvalResult {
        // The ARGUMENT SPINE is rooted for the whole call. It hangs off the form
        // being evaluated, which is rooted too — but an operative may park a
        // tail and let that form go, and then the spine is held in Rust alone.
        let mark = self.root_mark();
        self.root_push(args);

        // An operative takes the spine as written; everything else wants values.
        if let Body::Operative(f) = def.body {
            let out = f(self, args, env);
            self.root_truncate(mark);
            return out;
        }
        let mut vals = match self.eval_args(args, env) {
            Ok(v) => v,
            Err(c) => {
                self.root_truncate(mark);
                return Err(c);
            }
        };
        // The values are already rooted -- `eval_args` leaves them so, because
        // every applicative needs the same thing and one place is easier to keep
        // right than ninety.
        // PADDED, not checked. A body indexes the slots its arity declares, so
        // the slots must exist -- but a missing operand is nil, not an error.
        // x-engine-c raises "+: operand is nil" here; that is the same layer
        // violation, and copying it would import someone else's.
        vals.resize(def.arity.0.max(vals.len()), NIL);
        let out = match def.body {
            // Already handled above; the compiler cannot know that.
            Body::Operative(_) => unreachable!("operatives return early"),
            // Handed the object model and nothing else. It cannot evaluate, it
            // cannot see an environment, and it cannot read the input stream.
            Body::Value(f) => f(&mut self.objects, &vals),
            // The DYNAMIC base, as an argument: p_base flows through the call,
            // so a host-defined handler running under `(b eval …)` sees the
            // child. It is not derivable from the environment — a closure's
            // body frames chain to its definition env, which is the LEXICAL
            // base. The frame's base backpointer serves the collector and
            // introspection; the running base is this value.
            Body::Applicative(f) => {
                let base = self.base;
                f(self, base, &vals)
            }
            // The pure kinds: unwrap, apply the operator, re-box. This preamble
            // was repeated in eleven primitive bodies before the operator became
            // the primitive.
            // TOWER OPS: the registry gets first refusal, then the machine.
            Body::TowerBinop(op, f) => match self.op_try(op, vals[0], vals[1])? {
                Some(v) => Ok(v),
                None => {
                    let (x, y) = (self.objects.as_int(vals[0]), self.objects.as_int(vals[1]));
                    Ok(self.objects.int(f(x, y)))
                }
            },
            Body::TowerPred(op, f) => match self.op_try(op, vals[0], vals[1])? {
                Some(v) => Ok(v),
                None => {
                    let (x, y) = (self.objects.as_int(vals[0]), self.objects.as_int(vals[1]));
                    Ok(self.objects.truth(f(x, y)))
                }
            },
            Body::IntBinop(f) => {
                let (x, y) = (self.objects.as_int(vals[0]), self.objects.as_int(vals[1]));
                Ok(self.objects.int(f(x, y)))
            }
            Body::IntPred(f) => {
                let (x, y) = (self.objects.as_int(vals[0]), self.objects.as_int(vals[1]));
                Ok(self.objects.truth(f(x, y)))
            }
            Body::IntUnop(f) => {
                let x = self.objects.as_int(vals[0]);
                Ok(self.objects.int(f(x)))
            }
        };
        self.root_truncate(mark);
        out
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
    fn eval_args(&mut self, args: Obj, env: EnvId) -> Result<Vec<Obj>, Cond> {
        // Collected first so the iterator's borrow of the objects ends before
        // `eval` needs it mutably.
        let forms: Vec<Obj> = self.objects.list(args).collect();
        let mark = self.root_mark();
        for f in &forms {
            self.root_push(*f);
        }
        let mut out = Vec::with_capacity(forms.len());
        for f in forms {
            match self.eval(f, env) {
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

    /// Apply a closure. APPLICATIVE, and the first parameter is bound to the
    /// CLOSURE ITSELF — x-lang's self-passing convention, which is why every
    /// function in the conformance prelude is written with a leading `self`. It
    /// recurses without ever having been named.
    fn apply_closure(&mut self, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
        let params = self.objects.closure_params(callee);
        let body = self.objects.closure_body(callee);
        let defenv = self.objects.closure_env(callee);

        let vals = self.eval_args(args, env)?;

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
        let out = self.eval_body_tail(body, frame);
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
    fn apply_op(&mut self, callee: Obj, args: Obj, env: EnvId) -> EvalResult {
        let params = self.objects.op_params(callee);
        let envname = self.objects.op_envname(callee);
        let body = self.objects.op_body(callee);
        let defenv = self.objects.op_env(callee);

        // Arguments arrive AS WRITTEN, so the spine is bound to the names
        // directly; a name with no argument is nil.
        let given: Vec<Obj> = self.objects.list(args).collect();
        let frame = self.envs.push(&mut self.objects, defenv);
        let env_mark = self.env_root_mark();
        self.env_root_push(frame);
        self.bind_params(frame, params, &given);
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
        for f in rest {
            self.eval(*f, env)?;
        }
        Ok(self.park_tail(*last, env))
    }

    pub fn eval_body(&mut self, body: Obj, env: EnvId) -> EvalResult {
        // Collected first so the iterator's borrow of the objects ends before
        // `eval` needs it mutably.
        let forms: Vec<Obj> = self.objects.list(body).collect();
        let mut last = NIL;
        for f in forms {
            last = self.eval(f, env)?;
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

    /// Missing operands are nil, extra ones ignored. Not a check — a machine
    /// reads the slots its instruction declares.
    #[test]
    fn operands_are_padded_and_extras_dropped() {
        assert_eq!(int_of("(+ 1)"), 1);
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

    /// The frame hangs off the DEFINING environment, not the caller's. Dynamic
    /// scope would find the caller's `n` here and answer 2.
    #[test]
    fn a_closure_resolves_names_where_it_was_written() {
        assert_eq!(
            int_of("(def n 1) (def get (fn (self) n)) (def call (fn (self) (%seq (def n 2) (get)))) (call)"),
            1
        );
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
