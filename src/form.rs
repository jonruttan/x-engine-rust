//! Reading a FORM, with the library's reader macros in the loop.
//!
//! # Why this exists
//!
//! In the reference engine the reader IS the tokenizer: `x_token_analyse`
//! iterates the BASE'S TYPE-ALIST, and both the read-eval loop and `io read` go
//! through `x_token_read`. So a reader macro the library installs changes every
//! later read — `include` and the REPL included.
//!
//! This engine had two readers that never met: a hand-written one for its own
//! input, and the `tok` instructions for the library to drive. `lib/x/reader/
//! lit-reader.x` installed its quote macro correctly, onto a type the engine's
//! reader never consulted, so `'x` read as a SYMBOL NAMED `'x` and surfaced far
//! away as `Unbound SYMBOL ''str`.
//!
//! # The shape
//!
//! At every position where a form may begin, the registered analysers get first
//! refusal; the built-in syntax is the fallback. That ordering is what makes a
//! macro a macro — `'` must win over "symbol starting with a quote" — and it is
//! why the library installs its analysers ahead of the engine's own catch-all
//! rather than after it.
//!
//! The built-in syntax stays in Rust rather than being re-expressed as handlers
//! on the builtin types, which is where the reference puts it. That is a real
//! deviation and worth naming: what x-lang requires is that a library macro
//! affect the engine's reads, and it does. What the reference additionally gets
//! is the ability to REPLACE the built-in readers, which nothing in lib/ does
//! today — every push there adds a macro ahead of the engine's handler and keeps
//! it as the tail.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::obj::{Obj, NIL};
use crate::prims::tok::{analyse, handler, handler_list};
use crate::vocabulary::Family;

impl Engine {
    /// One form from the engine's own reader, or `None` at end of input.
    /// One form from the CURRENT source — the innermost file being loaded, or
    /// the program's own input when nothing is loading.
    ///
    /// Which source that is matters, and not only for tidiness: x-lang's reader
    /// handlers call `(io read)` to read the rest of a literal they have begun,
    /// and while an `include` is running the thing being read is the FILE.
    pub fn read_form(&mut self) -> Result<Option<Obj>, Cond> {
        self.with_source(|e, r| e.read_form_from(r))
    }

    /// One byte from the current source.
    pub fn read_byte(&mut self) -> Option<u8> {
        self.with_source(|_, r| r.next_byte())
    }

    /// Run `f` against the source currently being read, and put it back.
    ///
    /// Taken out and returned rather than borrowed, because reading may EVALUATE
    /// — a reader macro runs x-lang code, which can read further — and the
    /// engine cannot be borrowed twice.
    fn with_source<T>(&mut self, f: impl FnOnce(&mut Self, &mut crate::read::Reader) -> T) -> T {
        match self.loading.pop() {
            Some(mut r) => {
                let out = f(self, &mut r);
                self.loading.push(r);
                out
            }
            None => {
                let mut r = std::mem::replace(&mut self.reader, crate::read::Reader::new(""));
                let out = f(self, &mut r);
                self.reader = r;
                out
            }
        }
    }

    pub(crate) fn read_form_from(
        &mut self,
        r: &mut crate::read::Reader,
    ) -> Result<Option<Obj>, Cond> {
        r.skip_blanks();
        if r.peek().is_none() {
            return Ok(None);
        }
        if let Some(v) = self.try_macro(r)? {
            return Ok(Some(v));
        }
        if r.peek() == Some(b'(') {
            r.bump();
            return Ok(Some(self.read_list_form(r)?));
        }
        Ok(r.read_one_builtin(&mut self.objects))
    }

    /// A list, whose ELEMENTS are read as forms.
    ///
    /// That is the whole reason lists live here rather than in the lexer: every
    /// element is a position where a macro may begin, and `(def q 'str)` is the
    /// ordinary case. Reading elements with the bare lexer left macros working
    /// only at top level, which looks like working until the first quoted symbol
    /// inside a form.
    fn read_list_form(&mut self, r: &mut crate::read::Reader) -> Result<Obj, Cond> {
        // ROOTED AS THEY ARE READ. Elements accumulate in a Rust vector, and
        // reading a later one can run a READER MACRO — which is x-lang code,
        // which evaluates, which can collect. Everything read so far is then
        // reachable from nothing at all.
        //
        // This is the quiet one: it needs a macro inside a list, a collection
        // during it, and the list to be used afterwards. A boot does all three.
        let mark = self.root_mark();
        let mut items: Vec<Obj> = Vec::new();
        let mut tail = NIL;
        loop {
            r.skip_blanks();
            match r.peek() {
                None => break,
                Some(b')') => {
                    r.bump();
                    break;
                }
                // A lone `.` marks the tail. It is only special standing alone:
                // `.5` and `foo.bar` are ordinary atoms.
                Some(b'.') if r.at_dot_separator() && !items.is_empty() => {
                    r.bump();
                    if let Some(t) = self.read_form_from(r)? {
                        tail = t;
                        self.root_push(tail);
                    }
                    r.skip_blanks();
                    if r.peek() == Some(b')') {
                        r.bump();
                    }
                    break;
                }
                _ => match self.read_form_from(r)? {
                    Some(o) => {
                        self.root_push(o);
                        items.push(o);
                    }
                    None => break,
                },
            }
        }
        let mut out = tail;
        for &o in items.iter().rev() {
            out = self.objects.pair(o, out);
            self.root_push(out);
        }
        self.root_truncate(mark);
        Ok(out)
    }

    /// Give the registered analysers first refusal at this position.
    ///
    /// Every type in the base's type-alist is offered the position; the first to
    /// score wins, and its readers run against a buffer covering exactly the
    /// span it claimed. A type with no analyse handler — which is most of them —
    /// costs one nil check.
    fn try_macro(&mut self, r: &mut crate::read::Reader) -> Result<Option<Obj>, Cond> {
        let base = self.base;
        let alist = crate::base::get(&self.objects, base, crate::base::TYPE_ALIST);
        if alist.is_nil() {
            return Ok(None);
        }
        let types: Vec<Obj> = self
            .objects
            .list(alist)
            .map(|entry| self.objects.rest(entry))
            .collect();

        // ROOTED for the same reason the scorer's locals are: the analysers and
        // readers below are x-lang code and may collect, and the source object
        // and the buffers handed to them live only in Rust locals.
        let text = r.text_obj(&mut self.objects);
        let mark = self.root_mark();
        self.root_push(text);
        for t in &types {
            self.root_push(*t);
        }
        let at = r.pos() as u64;

        // ONE CONTEST, then read with the winner. See `prims::tok::analyse`.
        let (ty, claim) = match analyse(self, &types, text, at) {
            Ok(Some(w)) => w,
            Ok(None) => {
                self.root_truncate(mark);
                return Ok(None);
            }
            Err(c) => {
                self.root_truncate(mark);
                return Err(c);
            }
        };
        {
            // The magnitude is the span; the sign only ordered the contest.
            let n = claim.unsigned_abs();
            if n == 0 {
                self.root_truncate(mark);
                return Ok(None);
            }
            // The reader runs with the buffer positioned on the claimed span:
            // retain at the start, cursor at the end, so `buf last-char` is the
            // final character the analyser accepted.
            let reader = handler(self, ty, Family::Read);
            let env = self.root_env();
            for rd in handler_list(self, reader) {
                if rd.is_nil() {
                    continue;
                }
                let buf = self.objects.buf(text, at);
                self.objects.set_buf_cursor(buf, at + n);
                let bmark = self.root_mark();
                self.root_push(buf);
                self.root_push(rd);
                // The handler may read FURTHER through `tok read` — `'x` reads
                // the quote then the form after it — so the reader's position
                // comes from the buffer afterwards, not from the claim.
                let got = match self.call_with_values(rd, &[buf], env) {
                    Ok(v) => v,
                    Err(c) => {
                        self.root_truncate(mark);
                        return Err(c);
                    }
                };
                self.root_truncate(bmark);
                if !got.is_nil() {
                    r.set_pos(self.objects.buf_cursor(buf) as usize);
                    self.root_truncate(mark);
                    return Ok(Some(got));
                }
            }
        }
        self.root_truncate(mark);
        Ok(None)
    }

    /// Read one form from a BUFFER, leaving its cursor after what was read.
    ///
    /// This is `tok read`, and it is what a reader macro calls to read the form
    /// it prefixes: `%lit-read` answers `(lit X)` by reading X through here.
    pub fn read_form_at(&mut self, buf: Obj) -> Result<Obj, Cond> {
        let text = self.objects.buf_text(buf);
        let at = self.objects.buf_cursor(buf) as usize;
        let bytes = self.objects.bytes_of(text);
        let mut r = crate::read::Reader::from_bytes(bytes, at, text);
        let form = self.read_form_from(&mut r)?;
        self.objects.set_buf_cursor(buf, r.pos() as u64);
        Ok(form.unwrap_or(NIL))
    }
}
