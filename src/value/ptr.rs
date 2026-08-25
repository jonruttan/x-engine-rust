//! Pointers: an address wearing an object's clothes.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{Addr, Obj, Word};
use crate::objects::{Objects, FLAG_PTR};

impl Objects {
    pub fn ptr(&mut self, at: Addr) -> Obj {
        let o = self.alloc(FLAG_PTR, 1);
        self.set_data(o, 0, Word(at.raw()));
        o
    }

    /// Is this a POINTER object? Asked when marshalling for the foreign door,
    /// which converts a heap-internal address and leaves a process one alone.
    pub fn is_ptr(&self, o: Obj) -> bool {
        self.is(o, FLAG_PTR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pointer holds an ADDRESS in its data word, which is what `ptr ->obj`
    /// reads back — the round trip decision L1 rests on.
    #[test]
    fn a_pointer_carries_the_address_it_was_given() {
        let mut o = Objects::new();
        let target = o.int(7);
        let p = o.ptr(target.addr());
        assert_eq!(o.as_ptr(p), target.addr());
        assert_eq!(o.as_ptr(p).as_obj(), target);
    }

    /// An object used where a pointer is expected reads its own data word — no
    /// branch on whether it "is really" a pointer, because that would be a type
    /// judgement this layer does not make.
    #[test]
    fn a_non_pointer_operand_reads_its_data_word() {
        let mut o = Objects::new();
        let n = o.int(1234);
        assert_eq!(o.as_ptr(n).raw(), 1234);
    }
}
