//! The tokenizer's tape and its registered reader types.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{Obj, Word, NIL};
use crate::objects::{Objects, FLAG_BUF, FLAG_TOKBASE, FLAG_TOKEOF};

impl Objects {
    /// A buffer over `text`, with both marks at `at`.
    pub fn buf(&mut self, text: Obj, at: u64) -> Obj {
        let cursor = self.int(at as i64);
        let o = self.alloc(FLAG_BUF, 3);
        self.set_data(o, 0, Word(at));
        self.set_data(o, 1, cursor.word());
        self.set_data(o, 2, text.word());
        o
    }

    pub fn is_buf(&self, o: Obj) -> bool {
        self.is(o, FLAG_BUF)
    }

    pub fn buf_retain(&self, o: Obj) -> u64 {
        self.data(o, 0).raw()
    }

    pub fn set_buf_retain(&mut self, o: Obj, at: u64) {
        self.set_data(o, 0, Word(at))
    }

    /// The cursor lives in its own cell because the suite reaches it with
    /// `rest` and reads that object's word.
    pub fn buf_cursor(&self, o: Obj) -> u64 {
        self.data(self.data(o, 1).as_obj(), 0).raw()
    }

    pub fn set_buf_cursor(&mut self, o: Obj, at: u64) {
        let cell = self.data(o, 1).as_obj();
        self.set_data(cell, 0, Word(at))
    }

    pub fn buf_text(&self, o: Obj) -> Obj {
        self.data(o, 2).as_obj()
    }

    /// A base with NO types registered — deliberately bare, which is the whole
    /// purpose `base make-tok` exists for.
    pub fn tokbase(&mut self) -> Obj {
        self.alloc(FLAG_TOKBASE, 1)
    }

    pub fn is_tokbase(&self, o: Obj) -> bool {
        self.is(o, FLAG_TOKBASE)
    }

    pub fn tokbase_types(&self, o: Obj) -> Obj {
        self.data(o, 0).as_obj()
    }

    /// Registered types are kept in REGISTRATION ORDER, because the scorer must
    /// consider every one of them per position rather than stopping at the
    /// first: two types competing for the same character is the case that
    /// distinguishes a scorer from a search.
    pub fn tokbase_add(&mut self, o: Obj, ty: Obj) {
        let head = self.tokbase_types(o);
        let mut items: Vec<Obj> = self.list(head).collect();
        items.push(ty);
        let mut list = NIL;
        for &t in items.iter().rev() {
            list = self.pair(t, list);
        }
        self.set_data(o, 0, list.word());
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

    /// A fresh tokenizer base has NO types, which is the whole purpose of
    /// `base make-tok` existing beside `base make`.
    #[test]
    fn a_tokenizer_base_starts_bare_and_keeps_registration_order() {
        let mut o = Objects::new();
        let tb = o.tokbase();
        assert!(o.tokbase_types(tb).is_nil(), "born with no types");
        let (n1, n2) = (o.str_new("A"), o.str_new("B"));
        let (t1, t2) = (o.type_new(n1, NIL), o.type_new(n2, NIL));
        o.tokbase_add(tb, t1);
        o.tokbase_add(tb, t2);
        let types: Vec<Obj> = o.list(o.tokbase_types(tb)).collect();
        assert_eq!(types, vec![t1, t2], "registration order, not reversed");
    }
}
