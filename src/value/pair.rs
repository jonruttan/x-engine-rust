//! Pairs, and walking a spine.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::Obj;
use crate::objects::{Objects, FLAG_PAIR, FLAG_SPAIR};

impl Objects {
    /// A LIST pair — what x-lang's `pair` instruction makes and what `pair?`
    /// answers #t for.
    pub fn pair(&mut self, first: Obj, rest: Obj) -> Obj {
        self.cell(FLAG_PAIR, first, rest)
    }

    /// A STRUCTURAL pair: an interpreter spine. See [`FLAG_SPAIR`].
    ///
    /// Used for everything the ENGINE builds and the library walks reflectively
    /// rather than as data — the base, environment frames, type trees. It is not
    /// an optimisation and not a private convenience: the library reads the tag
    /// and behaves differently, so building a spine out of list pairs tells it
    /// the interpreter's own structure is a list.
    pub fn spair(&mut self, first: Obj, rest: Obj) -> Obj {
        self.cell(FLAG_SPAIR, first, rest)
    }

    fn cell(&mut self, flags: crate::obj::Flags, first: Obj, rest: Obj) -> Obj {
        let o = self.alloc(flags, 2);
        self.set_data(o, 0, first.word());
        self.set_data(o, 1, rest.word());
        o
    }

    /// Is this a pair of EITHER kind? For walking, where the tag is irrelevant.
    pub fn is_cell(&self, o: Obj) -> bool {
        self.is(o, FLAG_PAIR) || self.is(o, FLAG_SPAIR)
    }

    pub fn is_spair(&self, o: Obj) -> bool {
        self.is(o, FLAG_SPAIR)
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

    /// `pair?` answers #f for an interpreter SPINE, which is the whole point of
    /// the split.
    ///
    /// x-lang's library walks lists with `pair?` and reflects into the base with
    /// committed routes; a spine that answered #t would invite the list walkers
    /// into the interpreter's own structure. The reference states the rule in its
    /// ISA, of `base bind`: it "allocates a STRUCTURAL spair for the env spine,
    /// which X pair cannot make."
    #[test]
    fn a_spine_is_not_a_pair_but_is_still_walkable() {
        let mut o = Objects::new();
        let one = o.int(1);
        let two = o.int(2);
        let list = o.pair(one, two);
        let spine = o.spair(one, two);

        assert!(o.is_pair(list), "a list pair is a pair");
        assert!(!o.is_pair(spine), "a SPINE is not");

        // Both are walkable: same layout, different tag.
        assert!(o.is_cell(list) && o.is_cell(spine));
        assert_eq!(o.first(spine), one);
        assert_eq!(o.rest(spine), two);
    }

    /// The tags are distinct objects, not two names for one flag.
    #[test]
    fn the_two_kinds_are_told_apart() {
        let mut o = Objects::new();
        let list = o.pair(NIL, NIL);
        let spine = o.spair(NIL, NIL);
        assert!(o.is_spair(spine));
        assert!(!o.is_spair(list));
    }
}
