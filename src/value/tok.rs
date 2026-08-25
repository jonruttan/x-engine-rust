//! The tokenizer's tape and its registered reader types.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{Obj, Word};
use crate::objects::{Objects, FLAG_BUF, FLAG_BUFMARKS, FLAG_TOKEOF};

impl Objects {
    /// A buffer over `text`, with both marks at `at`.
    /// A READING buffer over `text`: everything already there is readable, so
    /// the WRITE mark sits at the end. This is the tokenizer's case.
    pub fn buf(&mut self, text: Obj, at: u64) -> Obj {
        let end = self.byte_len(text) as u64;
        let o = self.buf_writable(text, at, end);
        self.set_data(o, 3, Word(1));
        o
    }

    /// A buffer is `(val . (read . write))`, as the reference lays it out:
    /// `first(buffer)` is the val mark and `rest(buffer)` is the object whose
    /// first word is the read mark — the shape `lib/x/reader/intrinsics.x`
    /// walks with `%cell-int` and writes with `%buffer-unread`.
    ///
    /// Marks are OFFSETS into the text rather than raw pointers, and the text
    /// object and RO flag ride in slots past `rest`'s reach so the collector
    /// can root the region; both are invisible to x-lang.
    pub fn buf_writable(&mut self, text: Obj, at: u64, write: u64) -> Obj {
        let marks = self.alloc(FLAG_BUFMARKS, 2);
        self.set_data(marks, 0, Word(at));
        self.set_data(marks, 1, Word(write));
        let o = self.alloc(FLAG_BUF, 4);
        self.set_data(o, 0, Word(at));
        self.set_data(o, 1, marks.word());
        self.set_data(o, 2, text.word());
        self.set_data(o, 3, Word(0));
        o
    }

    pub fn is_buf(&self, o: Obj) -> bool {
        self.is(o, FLAG_BUF)
    }

    /// The val mark — the token start. `first(buffer)` in x-lang.
    pub fn buf_retain(&self, o: Obj) -> u64 {
        self.data(o, 0).raw()
    }

    pub fn set_buf_retain(&mut self, o: Obj, at: u64) {
        self.set_data(o, 0, Word(at))
    }

    /// The read mark, in the inner pair's first word — `(rest buffer)`'s cell.
    pub fn buf_cursor(&self, o: Obj) -> u64 {
        let marks = self.data(o, 1).as_obj();
        self.data(marks, 0).raw()
    }

    pub fn set_buf_cursor(&mut self, o: Obj, at: u64) {
        let marks = self.data(o, 1).as_obj();
        self.set_data(marks, 0, Word(at))
    }

    /// The write mark, in the inner pair's second word.
    pub fn buf_write(&self, o: Obj) -> u64 {
        let marks = self.data(o, 1).as_obj();
        self.data(marks, 1).raw()
    }

    pub fn set_buf_write(&mut self, o: Obj, at: u64) {
        let marks = self.data(o, 1).as_obj();
        self.set_data(marks, 1, Word(at))
    }

    /// Read-only: retain bumps the mark instead of compacting (#354).
    pub fn buf_ro(&self, o: Obj) -> bool {
        self.data(o, 3).raw() != 0
    }

    pub fn buf_text(&self, o: Obj) -> Obj {
        self.data(o, 2).as_obj()
    }

    /// The one end-of-input sentinel. Allocated once per engine, never compared
    /// by value: `lib/x/repl/loop.x` tests it with `(obj same?)` and says why --
    /// "eq? compares value words and could conflate a satom with an integer".
    /// The end-of-input sentinel, whose slot 0 holds its OWN ADDRESS.
    ///
    /// Not decoration. `eq?` compares the operand word of both sides without
    /// asking their types (see `prims::obj::eq`, and the reference's
    /// `x_prim_eq`), so a sentinel whose word were 0 would be `eq?` to the
    /// integer 0 — and `lib/x/repl/loop.x` reads a form, compares it against the
    /// sentinel, and would treat a literal `0` in the input as end-of-file.
    ///
    /// The reference has the same comparison and does not have the problem,
    /// because its sentinel is `x_token_eof_prim`, a prim whose word is a
    /// function pointer. An address is the same answer: unique, and never a
    /// small integer.
    pub fn token_eof(&mut self) -> Obj {
        let o = self.alloc(FLAG_TOKEOF, 1);
        self.set_data(o, 0, Word(o.word().raw()));
        o
    }

    pub fn is_token_eof(&self, o: Obj) -> bool {
        self.is(o, FLAG_TOKEOF)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buffer's layout is DICTATED by the suite's reflective reads: word 0
    /// is a raw retain mark and word 1 an object whose own word 0 is the cursor.
    #[test]
    fn the_marks_live_where_the_contract_puts_them() {
        let mut o = Objects::new();
        let text = o.str_new("abcd");
        let b = o.buf(text, 1);
        assert_eq!(o.buf_retain(b), 1);
        assert_eq!(o.buf_cursor(b), 1);
        assert_eq!(o.buf_text(b), text);
        // The cursor is reached with `rest`, as the analysers reach it.
        assert!(
            !o.rest(b).is_nil(),
            "the cursor must be an OBJECT, not a word"
        );
    }

    #[test]
    fn the_marks_move_independently() {
        let mut o = Objects::new();
        let text = o.str_new("abcd");
        let b = o.buf(text, 0);
        o.set_buf_cursor(b, 3);
        assert_eq!(o.buf_cursor(b), 3);
        assert_eq!(
            o.buf_retain(b),
            0,
            "advancing the cursor must not drag the mark"
        );
        o.set_buf_retain(b, 3);
        assert_eq!(o.buf_retain(b), 3);
    }
}
