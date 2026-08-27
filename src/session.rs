//! The process boundary.
//!
//! Mirrors the reference engine's `x-cli.c`: the program arrives on stdin, argv
//! is bound as a list, and forms are read and evaluated one at a time. Contract
//! layer E, and nothing else.
//!
//! It is `impl Engine` in its own file rather than code in `main`, so that an
//! embedder gets the same door the binary uses. A caller driving the engine
//! directly should not have to re-implement the read-eval loop to do it.

use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::{EnvId, Obj, NIL};

impl Engine {
    /// Hand the engine the program text. It keeps the reader so that `io read`
    /// and `io read-char` consume from the same stream.
    /// Read the program from the process's stdin, incrementally: a
    /// fixed region refills one byte at a time through the buffer row, as
    /// the reference's read-eval loop pulls through `x_type_buffer_read`.
    pub fn set_input_stdin(&mut self) {
        self.set_input_stream(Box::new(std::io::stdin()));
    }

    /// The same wiring over any byte stream — the test's injection door.
    pub fn set_input_stream(&mut self, input: Box<dyn std::io::Read>) {
        const CAP: usize = 1 << 18;
        let region = self.objects.str_make(CAP);
        let b = self.objects.buf_writable(region, 0, 0);
        let base = self.base;
        let bcell = self.objects.spair(b, NIL);
        crate::base::set(&mut self.objects, base, crate::base::BUFFER, bcell);
        let fd = self.objects.int(0);
        let fcell = self.objects.spair(fd, NIL);
        crate::base::set(&mut self.objects, base, crate::base::FILEIN, fcell);
        self.objects.input = Some(input);
        self.objects.input_cap = CAP as u64;
    }

    /// Compact the interactive source between top-level forms: the unread
    /// remainder moves to the region's front, which is what bounds a
    /// long-running session to the region's size.
    pub fn compact_input(&mut self) {
        let b = self.current_buffer();
        if b.is_nil() || self.objects.buf_ro(b) {
            return;
        }
        let c = self.objects.buf_cursor(b);
        if c == 0 {
            return;
        }
        let w = self.objects.buf_write(b);
        let text = self.objects.buf_text(b);
        let at = self.objects.str_bytes(text);
        for i in c..w {
            let v = self.objects.heap.byte(at.plus(i));
            self.objects.heap.set_byte(at.plus(i - c), v);
        }
        self.objects.set_buf_retain(b, 0);
        self.objects.set_buf_cursor(b, 0);
        self.objects.set_buf_write(b, w - c);
        self.objects.buf_line_shift(b, c);
    }

    pub fn set_input(&mut self, src: &str) {
        let text = self.objects.str_new(src);
        let b = self.objects.buf(text, 0);
        let base = self.base;
        let bcell = self.objects.spair(b, NIL);
        crate::base::set(&mut self.objects, base, crate::base::BUFFER, bcell);
        let fd = self.objects.int(0);
        let fcell = self.objects.spair(fd, NIL);
        crate::base::set(&mut self.objects, base, crate::base::FILEIN, fcell);
    }

    /// Push a source onto the base's input rows — the reference's include
    /// pushing (fd, line counter, read buffer) onto the base stacks.
    /// Push a source, stamping the buffer with the source-file id that
    /// `include` registered — every form read from it carries the id in its
    /// meta, which is what error-file reports.
    pub(crate) fn input_push_file(&mut self, src: &str, fd: i64, file_id: i64) {
        let text = self.objects.str_new(src);
        let b = self.objects.buf(text, 0);
        self.objects.set_buf_file_id(b, file_id);
        let base = self.base;
        let bhead = crate::base::get(&self.objects, base, crate::base::BUFFER);
        let bcell = self.objects.spair(b, bhead);
        crate::base::set(&mut self.objects, base, crate::base::BUFFER, bcell);
        let fdo = self.objects.int(fd);
        let fhead = crate::base::get(&self.objects, base, crate::base::FILEIN);
        let fcell = self.objects.spair(fdo, fhead);
        crate::base::set(&mut self.objects, base, crate::base::FILEIN, fcell);
        let line = crate::base::fresh_line_cell(&mut self.objects, 1);
        let lhead = crate::base::get(&self.objects, base, crate::base::LINE);
        crate::base::set(&mut self.objects, base, crate::base::LINE, line);
        self.line_stack.push(lhead);
    }

    pub(crate) fn input_pop(&mut self) {
        let base = self.base;
        let bhead = crate::base::get(&self.objects, base, crate::base::BUFFER);
        let brest = self.objects.rest(bhead);
        crate::base::set(&mut self.objects, base, crate::base::BUFFER, brest);
        let fhead = crate::base::get(&self.objects, base, crate::base::FILEIN);
        let frest = self.objects.rest(fhead);
        crate::base::set(&mut self.objects, base, crate::base::FILEIN, frest);
        if let Some(line) = self.line_stack.pop() {
            crate::base::set(&mut self.objects, base, crate::base::LINE, line);
        }
    }

    /// Read the next top-level form, or `None` at end of input.
    pub fn next_form(&mut self) -> Option<Obj> {
        self.read_form().ok().flatten()
    }

    /// Bind the argument vector as the list `args`. The engine parses NOTHING:
    /// it does not know what `--batch` or `--quiet` mean, and an engine that
    /// invented opinions about them would be implementing a protocol between the
    /// wrapper and the library that it has no part in.
    pub fn bind_args<S: AsRef<str>>(&mut self, argv: &[S]) {
        let mut list = NIL;
        for a in argv.iter().rev() {
            let s = self.objects.str_new(a.as_ref());
            list = self.objects.pair(s, list);
        }
        let name = self.objects.sym(crate::vocabulary::ARGS);
        let env = self.root_env();
        self.envs.bind(&mut self.objects, env, name, list);
    }

    /// Evaluate one top-level form in the global environment.
    /// Evaluate one TOP-LEVEL form: the root stack is truncated back to where
    /// it stood, keeping only the result.
    ///
    /// `eval` roots every result "until the enclosing evaluation moves on" —
    /// and at the top level nothing ever moved on, so every form's value stayed
    /// rooted for the life of the process. One integer per REPL line; thousands
    /// of dead results across a boot. Found by the live-count flatness test the
    /// heap-owned environments made possible: the old frame-count test could
    /// not see object growth at all.
    pub fn eval_top(&mut self, form: Obj) -> EvalResult {
        let env = self.root_env();
        // Top level owns the stack: the previous result is done with the
        // moment the next form evaluates, so only this result stays rooted.
        let out = self.eval(form, env);
        if self.active_evals == 0 {
            self.root_truncate(0);
        }
        if let Ok(v) = out {
            self.root_push(v);
        }
        out
    }

    /// Read and evaluate a whole source string, answering the last value. This is
    /// what makes the engine testable without a subprocess: a test states source
    /// and asserts on the object that comes back.
    ///
    /// ```
    /// use x_engine::engine::Engine;
    /// let mut e = Engine::new();
    /// let v = e.eval_str("(def x 7) (* x 6)").unwrap();
    /// assert_eq!(e.objects.as_int(v), 42);
    /// ```
    pub fn eval_str(&mut self, src: &str) -> EvalResult {
        let env = self.root_env();
        self.eval_source(src, env)
    }

    /// Read a source string and evaluate every form in it, answering the last.
    /// `include` and the test harness are the same act on different sources.
    /// Read a source string and evaluate every form in it, answering the last.
    ///
    /// Through the FORM reader, so an included file sees the same reader macros
    /// the top level does. Reading the whole file up front would be simpler and
    /// wrong: `lib/x/reader/lit-reader.x` installs the quote macro partway
    /// through x-core.x, and everything after it — in the same file — is written
    /// expecting `'x` to work.
    pub fn eval_source(&mut self, src: &str, env: EnvId) -> EvalResult {
        self.eval_source_fd(src, env, 0)
    }

    /// As `eval_source`, recording `fd` on the filein row for the duration —
    /// what `include` pushes for the file it opened.
    pub fn eval_source_fd(&mut self, src: &str, env: EnvId, fd: i64) -> EvalResult {
        self.eval_source_file(src, env, fd, 0)
    }

    /// As `eval_source_fd`, with the source-file id `include` registered.
    pub fn eval_source_file(&mut self, src: &str, env: EnvId, fd: i64, file_id: i64) -> EvalResult {
        // PUSHED as the current source, so `io read` inside a reader handler
        // reads from THIS text rather than from the process's input. The vector
        // reader in lib/x/type/vector.x does exactly that, and reaching past the
        // file ate a form off stdin — which is how the REPL launcher disappeared.
        self.input_push_file(src, fd, file_id);
        // Top level owns the root stack; the previous source's rooted result
        // is done with when another source arrives. Nested entry — include
        // evaluates a file mid-eval — must not touch it: the outer
        // evaluation's roots live below. The guard is `active_evals` because
        // `hide_pending` zeroes the save count by design (a loaded file's
        // defs must look top-level), so saves cannot carry this.
        if self.active_evals == 0 {
            self.root_truncate(0);
        }
        // ONE mark for the whole source: each form's evaluation truncates back
        // here and re-pushes only its own result, so the stack carries exactly
        // one value — the latest — however many forms the source holds. Taking
        // the mark per-form kept the previous push under it and grew the stack
        // by one root per top-level form for the life of the process.
        let mark = self.root_mark();
        let mut last = NIL;
        let out = loop {
            match self.read_form() {
                Ok(Some(form)) => match self.eval(form, env) {
                    Ok(v) => {
                        self.root_truncate(mark);
                        self.root_push(v);
                        last = v;
                    }
                    Err(c) => {
                        self.root_truncate(mark);
                        break Err(c);
                    }
                },
                Ok(None) => break Ok(last),
                Err(c) => break Err(c),
            }
        };
        self.input_pop();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;

    /// Contract layer E: every argv element is bound as the list `args`, and the
    /// engine parses NONE of it — `--batch` is just a string here.
    #[test]
    fn argv_is_bound_as_a_list_of_strings() {
        let mut e = Engine::new();
        e.bind_args(&["--batch", "file.x"]);
        let env = e.root_env();
        let name = e.objects.sym("args");
        let args = e.envs.lookup(&e.objects, env, name).expect("args bound");
        let items: Vec<Obj> = e.objects.list(args).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(e.objects.str_val(items[0]), "--batch");
        assert_eq!(e.objects.str_val(items[1]), "file.x");
    }

    #[test]
    fn no_arguments_bind_the_empty_list() {
        let mut e = Engine::new();
        let empty: [&str; 0] = [];
        e.bind_args(&empty);
        let env = e.root_env();
        let name = e.objects.sym("args");
        assert!(e
            .envs
            .lookup(&e.objects, env, name)
            .expect("bound")
            .is_nil());
    }

    /// Forms come out in order and the stream then ENDS, which is what stops the
    /// binary's loop.
    #[test]
    fn forms_are_read_in_order_and_then_the_stream_ends() {
        let mut e = Engine::new();
        e.set_input("1 2");
        let a = e.next_form().expect("first");
        let b = e.next_form().expect("second");
        assert_eq!(e.objects.as_int(a), 1);
        assert_eq!(e.objects.as_int(b), 2);
        assert!(e.next_form().is_none());
    }

    /// The reader is the ENGINE'S, so `io read-char` consumes what the loop has
    /// not yet reached — the reason it lives here rather than in main.
    #[test]
    fn the_input_stream_is_shared_with_the_engine() {
        let mut e = Engine::new();
        e.set_input("1 xyz");
        let _ = e.next_form().expect("the first form");
        assert_eq!(e.read_byte(), Some(b' '), "the rest is still there");
    }

    /// The interactive path: bytes arrive through the input stream one at a
    /// time, forms read as they complete, and end of input LATCHES — the
    /// filein head flips to the fd's bitwise complement, so later reads
    /// fail without another pull.
    #[test]
    fn a_streamed_input_refills_per_byte_and_latches_eof() {
        let mut e = Engine::new();
        e.set_input_stream(Box::new(std::io::Cursor::new(b"(+ 1 2) 7".to_vec())));
        let a = e.next_form().expect("first form");
        let v = e.eval_top(a).expect("evaluates");
        assert_eq!(e.objects.as_int(v), 3);
        e.compact_input();
        let b = e.next_form().expect("second form");
        assert_eq!(e.objects.as_int(b), 7);
        assert!(e.next_form().is_none(), "the stream ends");
        let row = crate::base::get(&e.objects, e.base, crate::base::FILEIN);
        let fd = e.objects.first(row);
        assert!(e.objects.as_int(fd) < 0, "EOF latched the filein head");
        assert_eq!(e.objects.as_int(fd), !0i64, "as the fd's complement");
    }

    #[test]
    fn top_level_forms_evaluate_in_the_engines_own_base() {
        let mut e = Engine::new();
        e.set_input("(def x 41) (+ x 1)");
        let mut last = NIL;
        while let Some(f) = e.next_form() {
            last = e.eval_top(f).expect("evaluates");
        }
        assert_eq!(e.objects.as_int(last), 42);
    }
}
