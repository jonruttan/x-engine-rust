//! The symbol table.
//!
//! Name to object, and nothing else. It is a separate part because interning is
//! a POLICY over object construction rather than a property of storage: the
//! objects can make a symbol object any time, and this decides that it should make
//! at most one per spelling.
//!
//! x-lang requires that policy. `eq?` on two spellings of one name must hold, so
//! symbol identity IS pointer identity, and `str ->sym` answering a fresh object
//! would compare false against the same name written as a literal while every
//! other test still passed.
//!
//! SHARED ACROSS BASES, deliberately. `base bind` hands a name from one base into
//! another and the receiving base must look it up under the object it was given,
//! so the two contexts have to agree on what the symbol `answer` is.

use crate::obj::Obj;
use std::collections::HashMap;

pub struct Symbols {
    table: HashMap<String, Obj>,
}

impl Symbols {
    pub fn new() -> Self {
        Symbols {
            table: HashMap::new(),
        }
    }

    /// The object already interned under this name, if any.
    pub fn get(&self, name: &str) -> Option<Obj> {
        self.table.get(name).copied()
    }

    /// Record the object for this name. The caller constructs it, because
    /// constructing objects is the objects's job, not this table's.
    pub fn put(&mut self, name: &str, o: Obj) {
        self.table.insert(name.to_string(), o);
    }
}

impl Default for Symbols {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obj::NIL;

    #[test]
    fn a_name_is_absent_until_it_is_put() {
        let mut s = Symbols::new();
        assert!(s.get("alpha").is_none());
        s.put("alpha", NIL);
        assert_eq!(s.get("alpha"), Some(NIL));
    }

    /// Distinct spellings are distinct entries; the table does not conflate.
    #[test]
    fn distinct_names_do_not_collide() {
        let mut s = Symbols::new();
        let alpha = crate::obj::Word(8).as_obj();
        s.put("alpha", alpha);
        assert_eq!(s.get("alpha"), Some(alpha));
        assert!(s.get("beta").is_none());
    }
}
