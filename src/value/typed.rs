//! The type registry, and iterators.
//!
//! A type object is a SPINE, not a record: x-lang walks it by name, so it needs
//! one cell per committed route reachable by `rest` steps from the object
//! itself.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{Flags, Obj, NIL};
use crate::objects::{Objects, FLAG_FOREIGN, FLAG_ITER, FLAG_PRIM};

/// The routes a type object commits to, in order. Names come from the library:
/// it resolves them by name, so they are not this engine's to choose.
pub const TYPE_ROUTES: &[&str] = &[
    "type-name",
    "type-cvt",
    "type-display",
    "type-display-stack",
    "type-io",
    "type-iter",
    "type-proc",
    "type-write",
    "type-write-stack",
];

impl Objects {
    /// A type object, as a SPINE.
    ///
    /// Not a two-word record. x-lang walks a type by name — `type-name`,
    /// `type-iter`, `type-display` and six more, all rooted at the type object
    /// itself and reached by `rest` steps — so a type must be a pair spine with
    /// one cell per committed route, exactly like a base.
    ///
    /// The order is the contract; it is declared in
    /// `tools/contract/base-paths.x` and checked by
    /// [`crate::base::tests`]-style route tests below.
    pub fn type_new(&mut self, name: Obj, handlers: Obj) -> Obj {
        let mut spine = self.pair(handlers, NIL);
        // slots 8..1, filled nil; slot 0 is the name.
        for _ in 1..crate::value::typed::TYPE_ROUTES.len() {
            spine = self.pair(NIL, spine);
        }
        spine = self.pair(name, spine);
        spine
    }

    /// The engine's own handlers — `analyse` and `read` for the reader — kept
    /// past the routes the library walks.
    pub fn type_handlers_of(&self, o: Obj) -> Obj {
        let mut at = o;
        for _ in 0..crate::value::typed::TYPE_ROUTES.len() {
            at = self.rest(at);
        }
        self.first(at)
    }

    /// An instance of a custom type: `n` data words, and a header type word
    /// pointing at the type object.
    pub fn instance(&mut self, t: Obj, n: usize) -> Obj {
        let o = self.alloc(Flags::new(0), n.max(1));
        self.set_type_word(o, t);
        o
    }

    pub fn iter(&mut self, step: Obj, state: Obj) -> Obj {
        let o = self.alloc(FLAG_ITER, 2);
        self.set_data(o, 0, step.word());
        self.set_data(o, 1, state.word());
        o
    }

    pub fn iter_step(&self, o: Obj) -> Obj {
        self.data(o, 0).as_obj()
    }

    pub fn iter_state(&self, o: Obj) -> Obj {
        self.data(o, 1).as_obj()
    }

    /// The type object of a value. A custom instance carries one in its header;
    /// everything else is keyed by flags, so repeated asks answer the same
    /// object — `(same? (type of 1) (type of 2))` must hold.
    pub fn type_of(&mut self, o: Obj) -> Obj {
        if o.is_nil() {
            return NIL;
        }
        let carried = self.type_of_word(o);
        if !carried.is_nil() {
            return carried;
        }
        let flags = self.reported_flags(o);
        if let Some(&t) = self.builtin_types.get(&flags) {
            return t;
        }
        // Named after nothing in particular: x-lang's names for these types come
        // from the library, which a bare engine has not loaded, and inventing one
        // would put a name into the language nothing else agrees with.
        let name = self.str_new("BUILTIN");
        let t = self.type_new(name, NIL);
        self.builtin_types.insert(flags, t);
        t
    }

    /// The flags a value's TYPE is keyed by, which are not always the flags it
    /// carries.
    ///
    /// A foreign callable is flagged apart from a primitive INTERNALLY, because
    /// their data words mean different things — a primitive's is an index into
    /// the instruction table, a foreign callable's is a real machine address, and
    /// dispatching on the wrong one segfaults instead of answering wrongly. That
    /// is a representation concern and it stops at the engine's edge.
    ///
    /// x-lang sees one type. `obj make-callable` is the JIT's door: it takes the
    /// address of code just emitted and hands back "something the evaluator will
    /// call", and the conformance case pins exactly that — the result's type is
    /// the type of a primitive. Reporting a second callable type would make the
    /// library's type dispatch miss every compiled function.
    fn reported_flags(&self, o: Obj) -> Flags {
        let f = self.flags(o);
        if f == FLAG_FOREIGN {
            FLAG_PRIM
        } else {
            f
        }
    }

    pub fn set_iter_state(&mut self, o: Obj, s: Obj) {
        self.set_data(o, 1, s.word())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obj::NIL;

    /// The JIT's door: a made callable must be of the SAME type as a primitive,
    /// or the library's dispatch misses every function it compiled.
    #[test]
    fn a_foreign_callable_reports_the_type_of_a_primitive() {
        let mut o = Objects::new();
        let f = o.foreign(0x1000);
        let p = o.prim(0);
        assert_eq!(o.type_of(f), o.type_of(p));
    }

    /// And they stay distinct INSIDE, because the data words mean different
    /// things — an index into the instruction table versus a machine address.
    #[test]
    fn but_they_remain_distinguishable_to_the_engine() {
        let mut o = Objects::new();
        let f = o.foreign(0x1000);
        assert!(o.is_foreign(f));
        assert!(!o.is_prim(f));
    }

    /// EVERY route a type declares must resolve on a type this engine builds.
    ///
    /// The same check as the base's, for the same reason: a spine shorter than
    /// its route list walks off the end and answers nil, which reads as "no
    /// value" rather than "no such route" — and the library would take the nil.
    #[test]
    fn every_type_route_resolves() {
        let mut o = Objects::new();
        let name = o.str_new("T");
        let ty = o.type_new(name, NIL);
        let mut at = ty;
        for (n, route) in TYPE_ROUTES.iter().enumerate() {
            assert!(
                !at.is_nil(),
                "route `{}` at {} steps walks off the end",
                route,
                n
            );
            at = o.rest(at);
        }
    }

    /// `type-name` is the first cell, so the name is what a walk of zero steps
    /// finds.
    #[test]
    fn type_name_is_the_first_route() {
        let mut o = Objects::new();
        let name = o.str_new("CONFORM");
        let ty = o.type_new(name, NIL);
        assert_eq!(o.first(ty), name);
    }

    /// The engine's own handlers sit PAST the routes the library walks, so
    /// adding a route does not silently shift them into a library slot.
    #[test]
    fn the_engines_handlers_sit_past_the_declared_routes() {
        let mut o = Objects::new();
        let name = o.str_new("T");
        let key = o.sym("analyse");
        let entry = o.pair(key, NIL);
        let handlers = o.pair(entry, NIL);
        let ty = o.type_new(name, handlers);
        assert_eq!(o.type_handlers_of(ty), handlers);
    }
}
