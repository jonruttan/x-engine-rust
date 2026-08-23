//! Numbers and characters, and reading a word as one.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{Addr, Obj, Word};
use crate::objects::{Objects, FLAG_CHAR, FLAG_INT};

impl Objects {
    pub fn int(&mut self, v: i64) -> Obj {
        let o = self.alloc(FLAG_INT, 1);
        self.set_data(o, 0, Word::from_i64(v));
        o
    }

    pub fn int_val(&self, o: Obj) -> i64 {
        self.data(o, 0).as_i64()
    }

    pub fn is_int(&self, o: Obj) -> bool {
        self.is(o, FLAG_INT)
    }

    /// A char. Its code lives in the data word; reading it back is an operand
    /// read like any other, so there is no accessor here — `Engine::as_char`
    /// takes the word.
    pub fn char_new(&mut self, c: u32) -> Obj {
        let o = self.alloc(FLAG_CHAR, 1);
        self.set_data(o, 0, Word(c as u64));
        o
    }
    // UNCHECKED, every one. An engine is a machine: it reads the word at a slot
    // and applies an operator. Deciding a word is "not a number" is a TYPE
    // judgement, and types are x-lang's, one layer up.

    /// The operand word, read as a machine integer.
    pub fn as_int(&self, o: Obj) -> i64 {
        self.data(o, 0).as_i64()
    }

    /// The operand word, read as a character code.
    pub fn as_char(&self, o: Obj) -> u32 {
        self.data(o, 0).raw() as u32
    }

    /// The operand word, read as a byte.
    pub fn as_byte(&self, o: Obj) -> u8 {
        self.data(o, 0).raw() as u8
    }

    /// The operand word, read as an address.
    ///
    /// No branch on whether the operand "is really" a pointer. `obj ->ptr` puts
    /// an object's address in the data word and `str ->ptr` puts a string's
    /// bytes there, so reading that word is the whole of it.
    pub fn as_ptr(&self, o: Obj) -> Addr {
        self.data(o, 0).as_addr()
    }
}

/// Walks a pair spine. The seven hand-written `while is_pair` loops this
/// replaces were the same five lines each time, and one of them got the
/// termination subtly different by using a nil sentinel where a pair could
/// legitimately be nil.
///
/// An `Iterator`, not a bespoke walker, so everything `Iterator` already offers
/// — `collect`, `map`, `count`, `zip` — comes with it rather than being
/// re-implemented per call site.
pub struct ListIter<'a> {
    objects: &'a Objects,
    at: Obj,
}

impl Iterator for ListIter<'_> {
    type Item = Obj;

    fn next(&mut self) -> Option<Obj> {
        if !self.objects.is_pair(self.at) {
            return None;
        }
        let v = self.objects.first(self.at);
        self.at = self.objects.rest(self.at);
        Some(v)
    }
}

impl Objects {
    /// The elements of a pair spine, in order. Stops at anything that is not a
    /// pair, so an improper tail simply ends the sequence.
    pub fn list(&self, head: Obj) -> ListIter<'_> {
        ListIter {
            objects: self,
            at: head,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_round_trip_including_negatives_and_extremes() {
        let mut o = Objects::new();
        for v in [0i64, 1, -1, i64::MAX, i64::MIN] {
            let n = o.int(v);
            assert_eq!(o.int_val(n), v, "{} did not survive", v);
        }
    }

    #[test]
    fn a_char_holds_its_code_point() {
        let mut o = Objects::new();
        let c = o.char_new(b'2' as u32);
        assert_eq!(o.as_char(c), 50);
    }

    /// Reading an operand is UNCHECKED: a char read as an integer answers its
    /// code, which is what makes `(< chr 48)` work on a character in an
    /// analyser without a conversion.
    #[test]
    fn a_char_read_as_an_integer_is_its_code() {
        let mut o = Objects::new();
        let c = o.char_new(b'a' as u32);
        assert_eq!(o.as_int(c), 97);
    }
}
