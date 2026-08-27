//! Reading a FORM, with the library's reader macros in the loop.
//!
//! # Why this exists
//!
//! In the reference engine the reader IS the tokenizer: `x_token_analyse`
//! iterates the BASE'S TYPE-ALIST, and both the read-eval loop and `io read` go
//! through `x_token_read`. So a reader macro the library installs changes every
//! later read — `include` and the REPL included.
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
        let b = self.current_buffer();
        if b.is_nil() {
            return Ok(None);
        }
        self.read_form_in(b)
    }

    /// One byte from the current source.
    pub fn read_byte(&mut self) -> Option<u8> {
        let b = self.current_buffer();
        if b.is_nil() {
            return None;
        }
        self.objects.buf_next_byte(b)
    }

    /// The buffer being read: the head of the base's buffer row — the
    /// reference's `x_base_field_buffer(p_base)`.
    pub(crate) fn current_buffer(&self) -> Obj {
        let row = crate::base::get(&self.objects, self.base, crate::base::BUFFER);
        if row.is_nil() {
            return crate::obj::NIL;
        }
        self.objects.first(row)
    }

    pub(crate) fn read_form_in(&mut self, b: Obj) -> Result<Option<Obj>, Cond> {
        // An interactive source prefetches to a line boundary, so the
        // analyser contest's bounded view holds every byte a token on this
        // line can claim — a per-byte refill alone left a contest seeing
        // one digit of an integer.
        self.objects.buf_prefetch_line(b);
        self.objects.buf_skip_blanks(b);
        if self.objects.buf_peek(b).is_none() {
            return Ok(None);
        }
        // The form's source location, taken where it BEGINS — stamped on
        // whatever comes back, as the reference's `x_token_read` stamps each
        // token. Meta words exist only while the policy cell arms them.
        let line = self.objects.buf_line(b);
        let file = self.objects.buf_file_id(b);
        let got = if let Some(v) = self.try_macro(b)? {
            v
        } else if self.objects.buf_peek(b) == Some(b'(') {
            self.objects.buf_bump(b);
            self.read_list_form(b)?
        } else if let Some(v) = self.objects.buf_read_one_builtin_except_atom(b) {
            v
        } else {
            self.read_atom_delimited(b)?
        };
        self.objects.stamp_meta(got, line, file);
        Ok(Some(got))
    }

    /// An atom scan that honours registered DELIMIT handlers: after each
    /// accepted byte the handlers are offered the position, and a claim ends
    /// the token there — which is what makes `'a'b` two tokens. The handler
    /// is given a bounded view whose last char is the candidate.
    fn read_atom_delimited(&mut self, b: Obj) -> Result<Obj, Cond> {
        let base = self.base;
        let alist = crate::base::get(&self.objects, base, crate::base::TYPE_ALIST);
        let mut delims: Vec<Obj> = Vec::new();
        let types: Vec<Obj> = self
            .objects
            .list(alist)
            .map(|entry| self.objects.rest(entry))
            .collect();
        for t in types {
            let slot = handler(self, t, Family::Delimit);
            for h in handler_list(self, slot) {
                if !h.is_nil() {
                    delims.push(h);
                }
            }
        }
        if delims.is_empty() {
            return Ok(self.objects.buf_read_atom(b));
        }
        let text = self.objects.buf_text(b);
        let start = self.objects.buf_cursor(b);
        let env = self.root_env();
        let mark = self.root_mark();
        for &h in &delims {
            self.root_push(h);
        }
        while let Some(c) = self.objects.buf_peek(b) {
            if c.is_ascii_whitespace() || c == b'(' || c == b')' || c == b';' {
                break;
            }
            let at = self.objects.buf_cursor(b);
            if at > start {
                // Offer the position: a view with the candidate as last char.
                let view = self.objects.buf(text, at + 1);
                self.objects.set_buf_retain(view, at);
                let vmark = self.root_mark();
                self.root_push(view);
                let mut claimed = false;
                for &h in &delims {
                    let got = match self.call_with_values(h, &[view], env) {
                        Ok(v) => v,
                        Err(c) => {
                            self.root_truncate(mark);
                            return Err(c);
                        }
                    };
                    if !got.is_nil() {
                        claimed = true;
                        break;
                    }
                }
                self.root_truncate(vmark);
                if claimed {
                    break;
                }
            }
            self.objects.buf_bump(b);
        }
        self.root_truncate(mark);
        let end = self.objects.buf_cursor(b);
        Ok(self.objects.buf_atom_from(b, start, end))
    }

    /// A list, whose ELEMENTS are read as forms.
    ///
    /// That is the whole reason lists live here rather than in the lexer: every
    /// element is a position where a macro may begin, and `(def q 'str)` is the
    /// ordinary case. Reading elements with the bare lexer left macros working
    /// only at top level, which looks like working until the first quoted symbol
    /// inside a form.
    fn read_list_form(&mut self, b: Obj) -> Result<Obj, Cond> {
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
            self.objects.buf_skip_blanks(b);
            match self.objects.buf_peek(b) {
                // End of input INSIDE a list is truncation, and it raises —
                // the reference's list reader errors on the EOF sentinel
                // rather than answering a partial list.
                None => {
                    self.root_truncate(mark);
                    let v = self.objects.str_new("Unterminated input");
                    return Err(Cond::Raised(v));
                }
                Some(b')') => {
                    self.objects.buf_bump(b);
                    break;
                }
                // A lone `.` marks the tail. It is only special standing alone:
                // `.5` and `foo.bar` are ordinary atoms. With no elements
                // before it, the list IS its tail: `( . x)` reads as the bare
                // form x.
                Some(b'.') if self.objects.buf_at_dot_separator(b) => {
                    self.objects.buf_bump(b);
                    if let Some(t) = self.read_form_in(b)? {
                        tail = t;
                        self.root_push(tail);
                    }
                    self.objects.buf_skip_blanks(b);
                    if self.objects.buf_peek(b) == Some(b')') {
                        self.objects.buf_bump(b);
                    }
                    break;
                }
                _ => match self.read_form_in(b)? {
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
    fn try_macro(&mut self, b: Obj) -> Result<Option<Obj>, Cond> {
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
        let text = self.objects.buf_text(b);
        let mark = self.root_mark();
        self.root_push(text);
        for t in &types {
            self.root_push(*t);
        }
        let at = self.objects.buf_cursor(b);

        // ONE CONTEST, then read with the winner. See `prims::tok::analyse`.
        // The contest runs on a BOUNDED view over the same text: the caller
        // prefetched the source to a line boundary, so the view holds every
        // byte a token on this line can claim.
        let env = self.root_env();
        let cbuf = self.objects.buf(text, at);
        self.root_push(cbuf);
        let (ty, claim) = match analyse(self, &types, cbuf, env) {
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
            // The reader runs on the CURRENT buffer, positioned on the
            // claimed span: retain at the start, cursor at the end, so
            // `buf last-char` is the final character the analyser accepted —
            // and a handler that reads FURTHER (`io read`, `tok read`)
            // continues from the claim's end on the same stream, as the
            // reference's shared base buffer does. A fresh buffer here left
            // the current one sitting on the claim, and a handler's nested
            // read met the same span again, claimed it again, and never
            // returned.
            let reader = handler(self, ty, Family::Read);
            let env = self.root_env();
            for rd in handler_list(self, reader) {
                if rd.is_nil() {
                    continue;
                }
                self.objects.set_buf_retain(b, at);
                self.objects.set_buf_cursor(b, at + n);
                let bmark = self.root_mark();
                self.root_push(rd);
                let got = match self.call_with_values(rd, &[b], env) {
                    Ok(v) => v,
                    Err(c) => {
                        self.root_truncate(mark);
                        return Err(c);
                    }
                };
                self.root_truncate(bmark);
                if !got.is_nil() {
                    self.root_truncate(mark);
                    return Ok(Some(got));
                }
                // A declining handler backs off the claim for the next one.
                self.objects.set_buf_cursor(b, at);
            }
        }
        self.objects.set_buf_cursor(b, at);
        self.root_truncate(mark);
        Ok(None)
    }

    /// Read one form from a BUFFER, leaving its cursor after what was read.
    ///
    /// This is `tok read`, and it is what a reader macro calls to read the form
    /// it prefixes: `%lit-read` answers `(lit X)` by reading X through here.
    pub fn read_form_at(&mut self, buf: Obj) -> Result<Obj, Cond> {
        let form = self.read_form_in(buf)?;
        Ok(form.unwrap_or(NIL))
    }
}
