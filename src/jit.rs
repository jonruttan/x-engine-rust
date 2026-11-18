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

use crate::diag::Cond;
use crate::engine::Engine;
use crate::obj::{Obj, Word};

impl Engine {
    /// The address an object crosses the door as; 0 for nil.
    pub(crate) fn jit_real(&self, o: Obj) -> u64 {
        o.word().raw()
    }

    /// The object a crossed value names: the word itself, as the reference
    /// passes it. A word outside the arena is a value too — a routine's
    /// address read out of a slot vector crosses this way — and every read
    /// through it answers 0, as the heap's reads do past the arena.
    pub(crate) fn jit_obj(&self, real: u64) -> Obj {
        Word(real).as_obj()
    }

    /// Run machine code: `call` enters it. A door that holds a raise while
    /// it runs leaves the code at once (the foreign crate's escape frame),
    /// and the raise is answered here, as the reference's longjmp lands at
    /// the guard. A raise held by an outer entry is kept aside and restored.
    ///
    /// The doors write the engine through the host pointer the foreign crate
    /// holds, which the compiler cannot see from `self`: left alone it treats
    /// `slot_error` as untouched across the call and reads back the None
    /// written before it. The engine's address is published to opaque code
    /// first, so the call is assumed to write it.
    ///
    /// An escape skips the compiled frames' unlinks from the root chain, as
    /// the reference's longjmp does, and its guard restores the chain head it
    /// saved: here the head is restored where the escape lands, or the next
    /// collect walks cells on a dead stack.
    pub(crate) fn enter_foreign(&mut self, call: impl FnOnce() -> u64) -> Result<u64, Cond> {
        let prev = self.slot_error.take();
        let base = self.base;
        let chain = self.root_chain_head(base);
        let me = std::hint::black_box(&mut *self);
        let out = call();
        match std::mem::replace(&mut me.slot_error, prev) {
            Some(c) => {
                me.set_root_chain_head(base, chain);
                Err(c)
            }
            None => Ok(out),
        }
    }

    /// The head of base `b`'s root chain: a machine-stack address, 0 empty.
    fn root_chain_head(&self, b: Obj) -> u64 {
        let field = crate::base::slot(&self.objects, b, crate::base::HEAP_ROOT_CHAIN);
        if field.is_nil() {
            return 0;
        }
        self.objects.data(field, 0).raw()
    }

    fn set_root_chain_head(&mut self, b: Obj, head: u64) {
        let field = crate::base::slot(&self.objects, b, crate::base::HEAP_ROOT_CHAIN);
        if !field.is_nil() {
            self.objects.set_data(field, 0, Word(head));
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
        // By NAME, as x-engine-c's x_obj_type_isprim asks: a prim made in the
        // parent is called from compiled code running in a child base, whose
        // PRIMITIVE type is a different object.
        let prim = crate::objects::kind_name(crate::objects::FLAG_PRIM);
        if !self.objects.is_type_named(callee, prim) {
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

    fn raise_held(&self) -> bool {
        self.slot_error.is_some()
    }
}
