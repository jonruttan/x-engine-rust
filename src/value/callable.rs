//! Everything that can sit at the head of a form, and environments as values.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{EnvId, Obj, Word};
use crate::objects::{
    Objects, FLAG_CONT, FLAG_ENV, FLAG_FN, FLAG_FOREIGN, FLAG_OP, FLAG_PRIM, FLAG_WRAP,
};

impl Objects {
    /// A primitive: the data word holds an index into the engine's prim table.
    pub fn prim(&mut self, idx: usize) -> Obj {
        let o = self.alloc(FLAG_PRIM, 1);
        self.set_data(o, 0, Word::from_usize(idx));
        o
    }

    pub fn is_prim(&self, o: Obj) -> bool {
        self.is(o, FLAG_PRIM)
    }

    pub fn prim_idx(&self, o: Obj) -> usize {
        self.data(o, 0).as_usize()
    }

    /// A foreign address, dressed as something callable.
    ///
    /// The word here is a REAL MACHINE ADDRESS, not an offset into this engine's
    /// heap — a `dlopen`/`dlsym` result, or an address `obj make-callable` was
    /// handed. It is a flag of its own rather than a primitive carrying a number
    /// because a primitive's data word is an INDEX into the instruction table,
    /// and an address would alias a real instruction.
    pub fn foreign(&mut self, addr: u64) -> Obj {
        let o = self.alloc(FLAG_FOREIGN, 1);
        self.set_data(o, 0, Word(addr));
        o
    }

    pub fn is_foreign(&self, o: Obj) -> bool {
        self.is(o, FLAG_FOREIGN)
    }

    pub fn foreign_addr(&self, o: Obj) -> u64 {
        self.data(o, 0).raw()
    }

    /// A closure: params, body, and the environment it was written in. Lexical
    /// scope is the whole reason the third word is here — a closure that looked
    /// its names up in the CALLER'S environment would be dynamic scope wearing
    /// the same syntax.
    pub fn closure(&mut self, params: Obj, body: Obj, env: EnvId) -> Obj {
        let o = self.alloc(FLAG_FN, 3);
        self.set_data(o, 0, params.word());
        self.set_data(o, 1, body.word());
        self.set_data(o, 2, env.word());
        o
    }

    pub fn is_closure(&self, o: Obj) -> bool {
        self.is(o, FLAG_FN)
    }

    pub fn closure_params(&self, o: Obj) -> Obj {
        self.data(o, 0).as_obj()
    }

    pub fn closure_body(&self, o: Obj) -> Obj {
        self.data(o, 1).as_obj()
    }

    pub fn closure_env(&self, o: Obj) -> EnvId {
        EnvId::from_word(self.data(o, 2))
    }

    /// An operative: params, the name its caller's environment binds to, body,
    /// and the environment it was written in.
    pub fn operative(&mut self, params: Obj, envname: Obj, body: Obj, env: EnvId) -> Obj {
        let o = self.alloc(FLAG_OP, 4);
        self.set_data(o, 0, params.word());
        self.set_data(o, 1, envname.word());
        self.set_data(o, 2, body.word());
        self.set_data(o, 3, env.word());
        o
    }

    pub fn is_op(&self, o: Obj) -> bool {
        self.is(o, FLAG_OP)
    }

    pub fn op_params(&self, o: Obj) -> Obj {
        self.data(o, 0).as_obj()
    }

    pub fn op_envname(&self, o: Obj) -> Obj {
        self.data(o, 1).as_obj()
    }

    pub fn op_body(&self, o: Obj) -> Obj {
        self.data(o, 2).as_obj()
    }

    pub fn op_env(&self, o: Obj) -> EnvId {
        EnvId::from_word(self.data(o, 3))
    }

    pub fn env_obj(&mut self, id: EnvId) -> Obj {
        let o = self.alloc(FLAG_ENV, 1);
        self.set_data(o, 0, id.word());
        o
    }

    pub fn is_env(&self, o: Obj) -> bool {
        self.is(o, FLAG_ENV)
    }

    pub fn env_id(&self, o: Obj) -> EnvId {
        EnvId::from_word(self.data(o, 0))
    }

    pub fn cont(&mut self, id: u64) -> Obj {
        let o = self.alloc(FLAG_CONT, 1);
        self.set_data(o, 0, Word(id));
        o
    }

    pub fn is_cont(&self, o: Obj) -> bool {
        self.is(o, FLAG_CONT)
    }

    pub fn cont_id(&self, o: Obj) -> u64 {
        self.data(o, 0).raw()
    }

    pub fn wrapper(&mut self, inner: Obj) -> Obj {
        let o = self.alloc(FLAG_WRAP, 1);
        self.set_data(o, 0, inner.word());
        o
    }

    pub fn is_wrapper(&self, o: Obj) -> bool {
        self.is(o, FLAG_WRAP)
    }

    pub fn wrapper_inner(&self, o: Obj) -> Obj {
        self.data(o, 0).as_obj()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obj::NIL;

    /// A closure's third word is an ENVIRONMENT, not an object. The typed
    /// accessors are what keep those apart: both are machine words, and a
    /// closure whose captured environment was really an object would bind names
    /// into arbitrary storage.
    #[test]
    fn a_closure_remembers_its_environment_id() {
        let mut o = Objects::new();
        let c = o.closure(NIL, NIL, EnvId::new(3));
        assert_eq!(o.closure_env(c), EnvId::new(3));
    }

    /// An operative carries the same, plus the name its caller's environment
    /// binds to.
    #[test]
    fn an_operative_carries_params_envname_body_and_env() {
        let mut o = Objects::new();
        let name = o.sym("e");
        let op = o.operative(NIL, name, NIL, EnvId::new(2));
        assert_eq!(o.op_envname(op), name);
        assert_eq!(o.op_env(op), EnvId::new(2));
        assert!(o.is_op(op));
        assert!(!o.is_closure(op), "an operative is not a closure");
    }

    /// A wrapper holds the operative ITSELF, which is what makes
    /// `(same? (unwrap (wrap o)) o)` hold.
    #[test]
    fn a_wrapper_holds_the_very_same_operative() {
        let mut o = Objects::new();
        let inner = o.operative(NIL, NIL, NIL, EnvId::new(0));
        let w = o.wrapper(inner);
        assert_eq!(o.wrapper_inner(w), inner);
    }

    /// A foreign callable is its OWN kind, not a primitive carrying an address.
    /// A primitive's data word is an index into the instruction table, so an
    /// address would alias a real instruction.
    #[test]
    fn a_foreign_callable_is_not_a_primitive() {
        let mut o = Objects::new();
        let f = o.foreign(4096);
        assert!(!o.is_prim(f), "or it would dispatch to instruction 4096");
    }

    /// And it reads back the address it was given, unchanged. The word is a REAL
    /// machine address, so a heap offset arriving here would be a segfault at the
    /// call rather than a wrong answer.
    #[test]
    fn a_foreign_callable_answers_the_address_it_was_given() {
        let mut o = Objects::new();
        let f = o.foreign(0xdead_beef);
        assert!(o.is_foreign(f));
        assert_eq!(o.foreign_addr(f), 0xdead_beef);
    }
}
