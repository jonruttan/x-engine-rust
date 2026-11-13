//! The engine's half of the JIT runtime door.
//!
//! Machine code the assembler lane emits calls nine C-ABI helpers, resolved
//! from the running binary with `dlsym(dlopen(NULL))`. The exported symbols
//! and their one unsafe dereference live in the foreign crate; this file is
//! the SAFE half — [`x_engine_foreign::JitHost`] implemented on the engine.
//!
//! Object references cross the door as they are: an object is its real
//! address in the pinned arena (law 7), and nil crosses as 0, as the
//! reference's NULL does.

use crate::engine::Engine;
use crate::obj::{Obj, Word, NIL};

impl Engine {
    /// The address an object crosses the door as; 0 for nil.
    pub(crate) fn jit_real(&self, o: Obj) -> u64 {
        o.word().raw()
    }

    /// The object a crossed value names; nil for 0 or an address outside
    /// the arena.
    pub(crate) fn jit_obj(&self, real: u64) -> Obj {
        match self.objects.heap.from_real(real) {
            Some(at) => at.as_obj(),
            None => NIL,
        }
    }
}

impl x_engine_foreign::JitHost for Engine {
    fn jit_mkint(&mut self, v: i64) -> u64 {
        let o = self.objects.int(v);
        self.jit_real(o)
    }

    fn jit_mkpair(&mut self, a: u64, b: u64) -> u64 {
        let first = self.jit_obj(a);
        let rest = self.jit_obj(b);
        let o = self.objects.pair(first, rest);
        self.jit_real(o)
    }

    fn jit_firstobj(&mut self, p: u64) -> u64 {
        let o = self.jit_obj(p);
        let v = self.objects.first(o);
        self.jit_real(v)
    }

    fn jit_restobj(&mut self, p: u64) -> u64 {
        let o = self.jit_obj(p);
        let v = self.objects.rest(o);
        self.jit_real(v)
    }

    fn jit_atomint(&mut self, p: u64) -> i64 {
        let o = self.jit_obj(p);
        self.objects.as_int(o)
    }

    /// Evaluate, as the reference's `x_eval_arg` does for a compiled
    /// function's argument. A raise cannot cross the machine code between
    /// here and the engine, so it is held on the engine and the argument
    /// answers nil; the door that entered the code raises it when the code
    /// returns, as the reference's longjmp would have. Once one is held,
    /// nothing more is evaluated: the reference's code would not have run on.
    fn jit_eval_arg(&mut self, expr: u64) -> u64 {
        if self.slot_error.is_some() {
            return 0;
        }
        let form = self.jit_obj(expr);
        let env = self.jit_env.unwrap_or_else(|| self.root_env());
        match self.eval(form, env) {
            Ok(v) => self.jit_real(v),
            Err(c) => {
                self.slot_error = Some(c);
                0
            }
        }
    }

    /// `score := sign * bufferlen`, answering the score — the tokenizer
    /// contest's accept, as `jit.c` words it.
    fn jit_score_set(&mut self, score: u64, sign: i64, buffer: u64) -> u64 {
        let s = self.jit_obj(score);
        let b = self.jit_obj(buffer);
        let len = self.objects.buf_cursor(b) as i64 - self.objects.buf_retain(b) as i64;
        self.objects.set_data(s, 0, Word((sign * len) as u64));
        score
    }

    fn jit_buffer_unread(&mut self, buffer: u64) -> u64 {
        let b = self.jit_obj(buffer);
        let at = self.objects.buf_cursor(b);
        self.objects.set_buf_cursor(b, at.saturating_sub(1));
        buffer
    }

    fn jit_buffer_len(&mut self, buffer: u64) -> i64 {
        let b = self.jit_obj(buffer);
        self.objects.buf_cursor(b) as i64 - self.objects.buf_retain(b) as i64
    }

    /// The byte the buffer read last, as the reference's
    /// `x_bufferlastchar` reads it: the byte before the read mark.
    fn jit_buffer_last_char(&mut self, buffer: u64) -> i64 {
        let b = self.jit_obj(buffer);
        let at = self.objects.buf_cursor(b);
        if at == 0 {
            return 0;
        }
        let text = self.objects.buf_text(b);
        let bytes = self.objects.str_bytes(text);
        self.objects.heap.byte(bytes.plus(at - 1)) as i64
    }

    /// The label an analyser accepted, written into the label cell the
    /// score's rest holds, as `jit.c` words it.
    fn jit_score_label(&mut self, score: u64, label: i64) -> u64 {
        let s = self.jit_obj(score);
        let cell = self.objects.rest(s);
        self.objects.set_data(cell, 0, Word(label as u64));
        score
    }

    /// Call a primitive the compiled code chose at run time: `args` is
    /// `(callee arg0 ...)`, the prim layout. A callee that is not a
    /// primitive raises, as the reference refuses to branch to it; a raise
    /// is held for the door that entered the code, as jit_eval_arg's is.
    fn jit_call_value(&mut self, args: u64) -> u64 {
        if self.slot_error.is_some() {
            return 0;
        }
        let call = self.jit_obj(args);
        let callee = self.objects.first(call);
        let base = self.objects.base;
        let prim_t = self
            .objects
            .builtin_type_in(base, crate::objects::FLAG_PRIM);
        if callee.is_nil() || self.objects.obj_type(callee) != prim_t {
            self.slot_error = Some(crate::diag::Cond::EngineMsg(
                crate::vocabulary::MSG_CALL_VALUE.to_string(),
            ));
            return 0;
        }
        let rest = self.objects.rest(call);
        let env = self.jit_env.unwrap_or_else(|| self.root_env());
        let out = match self.combine(callee, rest, env) {
            Some(r) => self.settle(r, env),
            None => Ok(callee),
        };
        match out {
            Ok(v) => self.jit_real(v),
            Err(c) => {
                self.slot_error = Some(c);
                0
            }
        }
    }

    /// A slot door entered from foreign code. A raise cannot cross the C
    /// frames between here and the engine, so it is held on the engine and
    /// the door answers nil; `slot_call` raises it when the foreign routine
    /// returns.
    fn slot_routine(&mut self, slot: usize, _base: u64, run: &mut [u64]) -> u64 {
        match self.slot_run(slot, run) {
            Ok(v) => v,
            Err(c) => {
                self.slot_error = Some(c);
                0
            }
        }
    }
}
