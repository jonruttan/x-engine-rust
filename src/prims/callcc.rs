//! Escape continuations.
//!
//! Mirrors the reference engine's `x-prim/callcc.c`. The boundary is not invented
//! here: x-engine-c drew it, and an engine that grouped these differently
//! would make the two implementations harder to read against each other for
//! no gain.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::{ContSnapshot, ControlRec, EvalResult};
use crate::obj::{EnvId, Obj, NIL};
use crate::prim::PrimDef;

/// `(call/cc f)` — ESCAPE-only continuations.
///
/// The continuation unwinds outward and cannot be re-entered. x-lang's library
/// never calls call/cc at all — only doc-prims.x documents it — so escape covers
/// everything the language does.
fn call_cc(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    e.with_escape(a[0], e.root_env())
}

/// `(%cc-invoke k v)` — begin an unwind that only k's own call/cc will stop.
fn cc_invoke(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    e.invoke_cont(a[0], a[1])
}

impl Engine {
    /// Begin an unwind that only its own `call/cc` will stop.
    pub fn invoke_cont(&mut self, k: Obj, v: Obj) -> EvalResult {
        let id = self.objects.cont_id(k);
        self.escaping = Some((id, v));
        Err(Cond::Raised(v))
    }

    /// Run `f` with a fresh escape continuation, catching only its own.
    ///
    /// The capture also SNAPSHOTS the control records in flight: a live
    /// invocation still unwinds to this frame, but one arriving after this
    /// frame has returned replays the snapshot at the top level instead —
    /// the re-entry the reference gets from copying its C stack.
    pub fn with_escape(&mut self, f: Obj, env: EnvId) -> EvalResult {
        let id = self.next_cont;
        self.next_cont += 1;
        let k = self.objects.cont(id);
        self.capture(id, k);
        match self.call_with_values(f, &[k], env) {
            Ok(v) => Ok(v),
            Err(e) => match self.escaping {
                // Ours: stop the unwind here and answer the thrown value.
                Some((eid, v)) if eid == id => {
                    self.escaping = None;
                    Ok(v)
                }
                _ => Err(e),
            },
        }
    }

    /// Is an escape passing through? `guard` asks, because catching one would
    /// strand it.
    pub fn is_escaping(&self) -> bool {
        self.escaping.is_some()
    }

    /// Did the records at capture time describe every in-flight frame?
    ///
    /// Each nested evaluation below the top-level form must have exactly one
    /// record — depths 1 through D-1, in order. A frame with none is an
    /// operative holding evaluation state in Rust locals the replay cannot
    /// see, so the capture is refused rather than resumed wrongly.
    fn covers_every_frame(recs: &[ControlRec], depth: u32) -> bool {
        if depth == 0 {
            return false;
        }
        if recs.len() as u32 != depth - 1 {
            return false;
        }
        recs.iter()
            .enumerate()
            .all(|(i, r)| r.depth() == i as u32 + 1)
    }

    /// An escape that reached the top with no live catcher: replay its
    /// snapshot if the capture covered every frame, else refuse catchably.
    /// Answers None for conditions that are not dead-extent escapes.
    pub(crate) fn resume_escape(&mut self, c: &Cond) -> Option<EvalResult> {
        let (id, v) = self.escaping?;
        let _ = c;
        self.escaping = None;
        let snap = match self.cont_snapshots.get(&id) {
            Some(s) if s.resumable => s.recs.clone(),
            _ => {
                return Some(Err(Cond::EngineMsg(
                    "continuation: extent is not resumable".to_string(),
                )));
            }
        };
        Some(self.replay(&snap, v))
    }

    /// Snapshot the control records for continuation `id` — what a
    /// dead-extent invocation will replay.
    fn capture(&mut self, id: u64, k: Obj) {
        let recs = self.control.clone();
        let resumable = Self::covers_every_frame(&recs, self.active_evals);
        self.cont_snapshots
            .insert(id, ContSnapshot { k, recs, resumable });
    }

    /// Rebuild the captured evaluation, inner record first, delivering `v`
    /// where `call/cc` originally returned.
    fn replay(&mut self, recs: &[ControlRec], v: Obj) -> EvalResult {
        recs.iter().rev().try_fold(v, |val, r| self.resume(r, val))
    }

    /// One record's remaining work, with the incoming value rooted across it.
    pub(crate) fn resume(&mut self, r: &ControlRec, val: Obj) -> EvalResult {
        let mark = self.root_mark();
        self.root_push(val);
        let out = match r {
            ControlRec::Pass { .. } => Ok(val),
            ControlRec::Bind { name, env, set, .. } => self.resume_bind(*name, *env, *set, val),
            ControlRec::Body { rest, env, .. } => self.resume_body(*rest, *env, val),
            ControlRec::Args {
                callee,
                done,
                rest,
                env,
                n,
                ..
            } => {
                let done = done.clone();
                self.resume_args(*callee, &done, *rest, *env, *n, val)
            }
        };
        self.root_truncate(mark);
        out
    }

    /// A `def`/`set!` receives its value; each form answers what it always
    /// answers — the name for `def`, the value for `set!`.
    fn resume_bind(&mut self, name: Obj, env: EnvId, set: bool, val: Obj) -> EvalResult {
        if set {
            if self.envs.set_existing(&mut self.objects, env, name, val) {
                Ok(val)
            } else {
                Err(Cond::Unbound(name))
            }
        } else {
            let target = if self.nothing_pending() {
                self.root_env()
            } else {
                env
            };
            self.envs.bind(&mut self.objects, target, name, val);
            Ok(name)
        }
    }

    /// A body resumes with its remaining forms; an exhausted one answers the
    /// value it was handed.
    fn resume_body(&mut self, rest: Obj, env: EnvId, val: Obj) -> EvalResult {
        if rest.is_nil() {
            Ok(val)
        } else {
            self.eval_body(rest, env)
        }
    }

    /// An argument list resumes by finishing its remaining forms and applying
    /// the callee to the full row.
    fn resume_args(
        &mut self,
        callee: Obj,
        done: &[Obj],
        rest: Obj,
        env: EnvId,
        n: usize,
        val: Obj,
    ) -> EvalResult {
        let mut vals = done.to_vec();
        vals.push(val);
        let mark = self.root_mark();
        for x in &vals {
            self.root_push(*x);
        }
        let more: Vec<Obj> = self.objects.list(rest).collect();
        for f in more {
            match self.eval(f, env) {
                Ok(x) => {
                    self.root_push(x);
                    vals.push(x);
                }
                Err(c) => {
                    self.root_truncate(mark);
                    return Err(c);
                }
            }
        }
        if vals.len() < n {
            vals.resize(n, NIL);
        }
        let out = self.call_with_values(callee, &vals, env);
        self.root_truncate(mark);
        out
    }
}

crate::uniform_engine!(call_cc_u, call_cc, 1);
crate::uniform_engine!(cc_invoke_u, cc_invoke, 2);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(Some("call/cc"), Some(("ctrl", "call/cc")), 1, call_cc_u),
    PrimDef::row(Some("%cc-invoke"), None, 2, cc_invoke_u),
];

#[cfg(test)]
mod tests {
    use crate::eval::ControlRec;
    use crate::obj::NIL;
    use crate::testkit::{eval_ok, int_of};

    /// The decomposition's point: each resumption is checkable alone,
    /// without a capture or an unwind anywhere in sight.
    #[test]
    fn a_bind_record_resumes_by_binding_the_delivered_value() {
        let (mut e, _) = eval_ok("(def probe 1)");
        let name = e.objects.sym("probe");
        let env = e.root_env();
        let v = e.objects.int(42);
        let rec = ControlRec::Bind {
            name,
            env,
            set: true,
            depth: 1,
        };
        let out = e.resume(&rec, v).expect("resumes");
        assert_eq!(e.objects.as_int(out), 42, "set! answers the value");
        let bound = e.envs.lookup(&e.objects, env, name).expect("bound");
        assert_eq!(e.objects.as_int(bound), 42, "and the binding took");
    }

    #[test]
    fn a_body_record_resumes_by_running_the_remaining_forms() {
        let (mut e, _) = eval_ok("(def a 1)");
        e.set_input("(set! a 5) (+ a 2)");
        let rest = {
            let f1 = e.next_form().expect("form 1");
            let f2 = e.next_form().expect("form 2");
            let tail = e.objects.pair(f2, NIL);
            e.objects.pair(f1, tail)
        };
        let env = e.root_env();
        let rec = ControlRec::Body {
            rest,
            env,
            depth: 1,
        };
        let v = e.objects.int(0);
        let out = e.resume(&rec, v).expect("resumes");
        assert_eq!(e.objects.as_int(out), 7, "the remaining forms ran in order");
    }

    #[test]
    fn an_exhausted_body_record_passes_the_value_through() {
        let (mut e, _) = eval_ok("1");
        let env = e.root_env();
        let rec = ControlRec::Body {
            rest: NIL,
            env,
            depth: 1,
        };
        let v = e.objects.int(9);
        let out = e.resume(&rec, v).expect("resumes");
        assert_eq!(e.objects.as_int(out), 9);
    }

    #[test]
    fn an_args_record_resumes_by_finishing_the_application() {
        // Captured "inside" the second argument of (+ 1 _ ): delivering 2
        // completes the row and applies the callee.
        let (mut e, _) = eval_ok("1");
        let plus = e.objects.sym("+");
        let env = e.root_env();
        let callee = e.envs.lookup(&e.objects, env, plus).expect("+ bound");
        let one = e.objects.int(1);
        let rec = ControlRec::Args {
            callee,
            done: vec![one],
            rest: NIL,
            env,
            n: 2,
            depth: 1,
        };
        let v = e.objects.int(2);
        let out = e.resume(&rec, v).expect("resumes");
        assert_eq!(e.objects.as_int(out), 3, "the application completed");
    }

    #[test]
    fn a_pass_record_is_the_identity() {
        let (mut e, _) = eval_ok("1");
        let rec = ControlRec::Pass { depth: 1 };
        let v = e.objects.int(4);
        let out = e.resume(&rec, v).expect("resumes");
        assert_eq!(e.objects.as_int(out), 4);
    }

    #[test]
    fn a_dead_extent_invocation_re_enters_through_the_records() {
        // BARE-engine spelling of the conformance check: the closure body is
        // the recorded spine, and after re-entry `cell` holds 42, whose
        // "application" echoes it (a non-callable head answers itself).
        let src = "((fn (self) (def cell ()) (set! cell (call/cc (fn (_ k) k))) (match (cell (cell 42)) (#t ())) cell))";
        assert_eq!(int_of(src), 42);
    }

    #[test]
    fn call_cc_returns_its_bodys_value_when_the_continuation_is_unused() {
        assert_eq!(int_of("(call/cc (fn (self k) 7))"), 7);
    }

    #[test]
    fn call_cc_gives_an_escape_continuation() {
        assert_eq!(int_of("(call/cc (fn (self k) (+ 1 (k 9))))"), 9);
    }

    /// THE SUBTLE ONE. An escaping continuation is not a condition, and a guard
    /// between the throw and its call/cc must let it through. Catching it would
    /// strand the escape at the wrong depth and silently turn a non-local exit
    /// into a handled error — the sort of bug that leaves every individual test
    /// passing.
    #[test]
    fn a_guard_does_not_catch_an_escaping_continuation() {
        assert_eq!(
            int_of("(call/cc (fn (self k) (guard (e 111) (k 9))))"),
            9,
            "the guard must not swallow the escape"
        );
    }

    /// And it still catches ordinary conditions raised inside the same call/cc.
    #[test]
    fn a_guard_still_catches_a_raise_inside_call_cc() {
        assert_eq!(
            int_of("(call/cc (fn (self k) (guard (e 111) (error 1))))"),
            111
        );
    }

    /// Two continuations do not catch each other's escapes.
    #[test]
    fn an_escape_passes_through_an_inner_call_cc() {
        assert_eq!(
            int_of("(call/cc (fn (self outer) (call/cc (fn (s2 inner) (outer 5)))))"),
            5
        );
    }
}
