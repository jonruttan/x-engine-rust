//! Pairs, and walking a spine.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::Obj;
use crate::objects::{Objects, FLAG_PAIR};

impl Objects {
    pub fn pair(&mut self, first: Obj, rest: Obj) -> Obj {
        let o = self.alloc(FLAG_PAIR, 2);
        self.set_data(o, 0, first.word());
        self.set_data(o, 1, rest.word());
        o
    }

    /// UNCHECKED, by x-lang's ruling: `first` and `rest` are undefined on a
    /// non-pair and are guarded at the call site, never here.
    ///
    /// This is not pedantry about a rule — the conformance suite reads a custom
    /// instance's payload with a plain `first`, so an implementation that checked
    /// for a pair would refuse the language's own object protocol. Nil needs no
    /// special case here: `data` already answers zero for it, because nil has no
    /// slots to read.
    pub fn first(&self, o: Obj) -> Obj {
        self.data(o, 0).as_obj()
    }

    pub fn rest(&self, o: Obj) -> Obj {
        self.data(o, 1).as_obj()
    }

    pub fn is_pair(&self, o: Obj) -> bool {
        self.is(o, FLAG_PAIR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obj::NIL;

    #[test]
    fn a_pair_holds_what_it_was_given() {
        let mut o = Objects::new();
        let (a, b) = (o.int(1), o.int(2));
        let p = o.pair(a, b);
        assert_eq!(o.first(p), a);
        assert_eq!(o.rest(p), b);
        assert!(o.is_pair(p));
    }

    /// The iterator stops at anything that is not a pair, so an IMPROPER tail
    /// simply ends the sequence rather than being yielded as an element.
    #[test]
    fn the_walk_stops_at_an_improper_tail() {
        let mut o = Objects::new();
        let (a, b) = (o.int(1), o.sym("tail"));
        let p = o.pair(a, b);
        let items: Vec<Obj> = o.list(p).collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], a);
    }

    #[test]
    fn walking_nil_yields_nothing() {
        let o = Objects::new();
        assert_eq!(o.list(NIL).count(), 0);
    }
}
