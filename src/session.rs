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
use crate::read::Reader;

impl Engine {
    /// Hand the engine the program text. It keeps the reader so that `io read`
    /// and `io read-char` consume from the same stream.
    pub fn set_input(&mut self, src: &str) {
        self.reader = Reader::new(src);
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
        // PUSHED as the current source, so `io read` inside a reader handler
        // reads from THIS text rather than from the process's input. The vector
        // reader in lib/x/type/vector.x does exactly that, and reaching past the
        // file ate a form off stdin — which is how the REPL launcher disappeared.
        self.loading.push(Reader::new(src));
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
        self.loading.pop();
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
        assert_eq!(e.reader.next_byte(), Some(b' '), "the rest is still there");
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
