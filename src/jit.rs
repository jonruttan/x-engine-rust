//! The engine's half of the JIT runtime door.
//!
//! Machine code the assembler lane emits calls nine C-ABI helpers, resolved
//! from the running binary with `dlsym(dlopen(NULL))`. The exported symbols
//! and their one unsafe dereference live in the foreign crate; this file is
//! the SAFE half — [`x_engine_foreign::JitHost`] implemented on the engine.
//!
//! Object references cross the door as REAL machine addresses, which the
//! PINNED arena makes stable (law 7): a real address maps back to an arena
//! offset with `from_real`, and an offset forward with `address_of`. Nil
//! crosses as 0 in both directions, as the reference's NULL does.

use crate::engine::Engine;
use crate::obj::{Obj, Word, NIL};

impl Engine {
    /// The real address an object crosses the door as; 0 for nil.
    fn jit_real(&self, o: Obj) -> u64 {
        if o.is_nil() {
            return 0;
        }
        self.objects.heap.address_of(o.addr())
    }

    /// The object a crossed real address names; nil for 0 or an address
    /// outside the arena.
    fn jit_obj(&self, real: u64) -> Obj {
        if real == 0 {
            return NIL;
        }
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
    /// analyser's re-entry. A condition cannot cross machine code, so an
    /// erroring argument answers nil.
    fn jit_eval_arg(&mut self, expr: u64) -> u64 {
        let form = self.jit_obj(expr);
        let env = self.jit_env.unwrap_or_else(|| self.root_env());
        match self.eval(form, env) {
            Ok(v) => self.jit_real(v),
            Err(_) => 0,
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
}
