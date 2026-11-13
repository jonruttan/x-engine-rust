//! The base's slot vector: the engine's routines, each at a fixed position,
//! called through the position. The reference's `x-eval-slots.h`.
//!
//! A base is an object of two units: the spine first, the slot vector second.
//! The vector's first unit holds its length, and slot I is its unit I + 1: a
//! word holding the address of a routine with the engine's one signature,
//! `(base, run) -> value`, where the run is a run of words, one an argument,
//! as the slot's row in `tools/contract/base-slots.x` gives them.
//!
//! Every base is born with the slot doors of the foreign crate in its slots:
//! C-ABI entries that run this engine's own routine. A language reads a slot
//! and calls it through `ptr call`, stores another routine over it with
//! `obj set!`, and the routine it stored calls the one it replaced. The engine
//! calls through the slot: when it holds the engine's own door the routine is
//! run directly, and anything else is called through its address.
//!
//! Object references cross a door as REAL addresses, and a door accepts either
//! currency, as the JIT door does (`jit.rs`).

use crate::diag::Cond;
use crate::engine::Engine;
use crate::obj::{Obj, Word, NIL};

pub const EVAL: usize = 0;
pub const EVAL_LIST: usize = 1;
pub const EVAL_BODY: usize = 2;
pub const EVAL_BODY_TCO: usize = 3;
pub const EVAL_TCO_TRAMPOLINE: usize = 4;
pub const EVAL_OP_BODY: usize = 5;
pub const CALLABLE_CALL: usize = 6;
pub const CALLABLE_APPLY: usize = 7;
pub const OBJ_PRIM_CALL: usize = 8;
pub const ENV_LOOKUP: usize = 9;
pub const ENV_BIND: usize = 10;
pub const ENV_EXTEND: usize = 11;
pub const ALIST_BST_LOOKUP: usize = 12;
pub const TOKEN_READ: usize = 13;
pub const TOKEN_ANALYSE: usize = 14;
pub const TOKEN_DELIMIT: usize = 15;
pub const HEAP_MARK: usize = 16;
pub const HEAP_SWEEP: usize = 17;
pub const OBJ_ALLOC: usize = 18;

/// The length of the slot vector.
pub const LEN: usize = 19;

/// The slot names, by position, as tools/contract/base-slots.x lists them.
pub const NAMES: [&str; LEN] = [
    "eval",
    "eval-list",
    "eval-body",
    "eval-body-tco",
    "eval-tco-trampoline",
    "eval-op-body",
    "callable-call",
    "callable-apply",
    "obj-prim-call",
    "env-lookup",
    "env-bind",
    "env-extend",
    "alist-bst-lookup",
    "token-read",
    "token-analyse",
    "token-delimit",
    "heap-mark",
    "heap-sweep",
    "obj-alloc",
];

/// A fresh slot vector holding the engine's own doors.
pub fn make(o: &mut crate::objects::Objects) -> Obj {
    let doors = x_engine_foreign::slot_doors();
    let v = o.alloc(crate::objects::FLAG_SLOTS, LEN + 1);
    let len = o.int(LEN as i64);
    o.set_data(v, 0, len.word());
    for (i, d) in doors.iter().enumerate() {
        o.set_data(v, (i + 1) as u64, Word(*d as u64));
    }
    v
}

impl Engine {
    /// The word in slot `slot` of the running base's vector.
    pub(crate) fn slot_word(&self, slot: usize) -> u64 {
        let v = self.objects.data(self.base, 1).as_obj();
        if v.is_nil() {
            return 0;
        }
        self.objects.data(v, (slot + 1) as u64).raw()
    }

    /// The routine a language stored in slot `slot`, or None while the slot
    /// holds the engine's own door.
    pub(crate) fn slot_replaced(&self, slot: usize) -> Option<u64> {
        let w = self.slot_word(slot);
        if w == 0 || w == x_engine_foreign::slot_doors()[slot] as u64 {
            None
        } else {
            Some(w)
        }
    }

    /// Call through slot `slot` with the run `words`: the engine's own
    /// routine directly, or the stored routine through its address.
    pub(crate) fn slot_call(&mut self, slot: usize, words: &mut [u64]) -> Result<u64, Cond> {
        match self.slot_replaced(slot) {
            None => self.slot_run(slot, words),
            Some(addr) => {
                let base = self.jit_real(self.base);
                let prev = self.slot_error.take();
                let out =
                    x_engine_foreign::call_routine(x_engine_foreign::Foreign(addr), base, words);
                match std::mem::replace(&mut self.slot_error, prev) {
                    Some(c) => Err(c),
                    None => Ok(out),
                }
            }
        }
    }

    /// An object crossing a door, in either currency.
    pub(crate) fn slot_obj(&self, w: u64) -> Obj {
        self.jit_obj(w)
    }

    /// The engine's own routine for each slot, over a run of words.
    pub(crate) fn slot_run(&mut self, slot: usize, run: &mut [u64]) -> Result<u64, Cond> {
        let env = self.jit_env.unwrap_or_else(|| self.root_env());
        let arg = |e: &Engine, i: usize| -> Obj {
            match run.get(i) {
                Some(w) => e.slot_obj(*w),
                None => NIL,
            }
        };
        let out: Obj = match slot {
            EVAL => {
                let x = arg(self, 0);
                let r = self.eval(x, env);
                self.settle(r, env)?
            }
            EVAL_LIST => {
                let args = arg(self, 0);
                let vals = self.eval_args(args, env)?;
                self.value_spine(&vals)
            }
            EVAL_BODY | EVAL_BODY_TCO | EVAL_OP_BODY => {
                let body = arg(self, 0);
                self.eval_body(body, env)?
            }
            EVAL_TCO_TRAMPOLINE => {
                let v = arg(self, 0);
                self.settle(Ok(v), env)?
            }
            CALLABLE_CALL => {
                let call = arg(self, 0);
                let f = self.objects.first(call);
                let args = self.objects.rest(call);
                self.eval_call(f, args, env)?
            }
            CALLABLE_APPLY | OBJ_PRIM_CALL => {
                let call = arg(self, 0);
                let f = self.objects.first(call);
                let vals: Vec<Obj> = self.objects.list(self.objects.rest(call)).collect();
                self.apply_values_own(f, &vals, env)?
            }
            ENV_LOOKUP => {
                let (e, s) = (arg(self, 0), arg(self, 1));
                self.envs
                    .lookup_entry(&self.objects, crate::obj::EnvId::from_obj(e), s)
            }
            ENV_BIND => {
                let (e, s, v) = (arg(self, 0), arg(self, 1), arg(self, 2));
                self.envs
                    .bind(&mut self.objects, crate::obj::EnvId::from_obj(e), s, v);
                v
            }
            ENV_EXTEND => {
                let (parent, params, vals) = (arg(self, 0), arg(self, 1), arg(self, 2));
                self.env_extend(crate::obj::EnvId::from_obj(parent), params, vals)
                    .obj()
            }
            ALIST_BST_LOOKUP => {
                let (tree, s) = (arg(self, 0), arg(self, 1));
                crate::env::Envs::tree_lookup(&self.objects, tree, s)
            }
            HEAP_MARK => {
                self.mark_phase();
                NIL
            }
            HEAP_SWEEP => {
                self.sweep_phase();
                NIL
            }
            OBJ_ALLOC => {
                let ty = arg(self, 0);
                let units = run.get(2).copied().unwrap_or(0) as usize;
                self.objects.instance(ty, units)
            }
            _ => {
                return Err(Cond::EngineMsg(format!(
                    "slot {}: this engine has no routine for it",
                    NAMES.get(slot).copied().unwrap_or("?")
                )))
            }
        };
        Ok(self.jit_real(out))
    }

    /// The reference's `x_env_extend`: a child of `parent` binding each
    /// parameter to its value in order, a missing value nil, and a symbol in
    /// place of the rest of the list binding the rest of the values.
    pub(crate) fn env_extend(
        &mut self,
        parent: crate::obj::EnvId,
        params: Obj,
        vals: Obj,
    ) -> crate::obj::EnvId {
        let env = self.envs.push(&mut self.objects, parent);
        let (mut p, mut v) = (params, vals);
        while !p.is_nil() {
            if self.objects.is_sym(p) {
                self.envs.bind(&mut self.objects, env, p, v);
                return env;
            }
            let val = if v.is_nil() {
                NIL
            } else {
                self.objects.first(v)
            };
            let name = self.objects.first(p);
            self.envs.bind(&mut self.objects, env, name, val);
            p = self.objects.rest(p);
            v = if v.is_nil() {
                NIL
            } else {
                self.objects.rest(v)
            };
        }
        env
    }
}
