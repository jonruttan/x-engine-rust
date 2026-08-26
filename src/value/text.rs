//! Strings and symbols.
//!
//! An `impl Objects` block of its own. The objects is one type because objects
//! share a header and an allocator; it is not one FILE, because these kinds
//! have nothing else to do with each other.

use crate::obj::{Addr, Obj, Word};
use crate::objects::{Objects, FLAG_STR, FLAG_SYM};

const ERROR_SCRATCH: usize = 255;

impl Objects {
    /// A string object VIEWING bytes already in the objects, allocating no storage
    /// of its own. x-lang's rule is to wrap rather than reallocate, and
    /// `ptr ->str` is the instruction that says so.
    pub fn str_at(&mut self, at: Addr) -> Obj {
        let o = self.alloc(FLAG_STR, 1);
        self.set_data(o, 0, Word(at.raw()));
        o
    }

    pub fn str_new(&mut self, s: &str) -> Obj {
        let at = self.heap.store_bytes(s.as_bytes());
        self.str_at(at)
    }

    /// `str make n`: a fresh n-byte region, space-filled and NUL-terminated so a
    /// byte-length read sees n. NOT promised to be zeroed.
    pub fn str_make(&mut self, n: usize) -> Obj {
        let at = self.heap.alloc_bytes(n + 1);
        self.heap.fill_bytes(at, b' ', n as u64);
        self.heap.set_byte(at.plus(n as u64), 0);
        self.str_at(at)
    }

    /// Where a string's bytes live — what `str ->ptr` answers.
    pub fn str_bytes(&self, o: Obj) -> Addr {
        self.data(o, 0).as_addr()
    }

    /// Bytes up to the NUL. This is where the C-string ruling becomes observable.
    pub fn byte_len(&self, o: Obj) -> usize {
        self.bytes_of(o).len()
    }

    /// The bytes of a string, up to its NUL.
    pub fn bytes_of(&self, o: Obj) -> Vec<u8> {
        self.heap.bytes_at(self.str_bytes(o))
    }

    /// Heap a run of bytes as a fresh NUL-terminated string.
    pub fn str_from_bytes(&mut self, bytes: &[u8]) -> Obj {
        let at = self.heap.store_bytes(bytes);
        self.str_at(at)
    }

    /// Read a NUL-terminated byte run back out.
    pub fn str_val(&self, o: Obj) -> String {
        String::from_utf8_lossy(&self.bytes_of(o)).into_owned()
    }

    pub fn is_str(&self, o: Obj) -> bool {
        self.is(o, FLAG_STR)
    }

    /// Interned symbol. Two spellings of one name are the SAME object, which is
    /// what makes `eq?` on symbols a pointer comparison.
    /// A type HANDLE: an atom holding a name, carrying the atom tag.
    ///
    /// Deliberately NOT interned with the symbols. A handle and a symbol of the
    /// same spelling are different objects, because they carry different tags
    /// and the library reads the tag: `%reflect-handle-tw?` asks whether a word
    /// marks a HANDLE, and an interned symbol must answer no.
    /// The engine-wide "BASE" tag, made once.
    pub(crate) fn base_tag(&mut self) -> Obj {
        if self.base_tag_atom.is_nil() {
            self.base_tag_atom = self.handle("BASE");
        }
        self.base_tag_atom
    }

    /// A fresh error-scratch atom over its own byte region — the base's
    /// error-str row. Engine-raised conditions write their message into the
    /// region and raise the atom itself.
    pub(crate) fn error_atom(&mut self) -> Obj {
        let at = self.heap.alloc_bytes(ERROR_SCRATCH + 1);
        self.heap.set_byte(at, 0);
        let o = self.alloc(crate::objects::FLAG_HANDLE, 1);
        self.set_data(o, 0, Word(at.raw()));
        o
    }

    /// Write a message into an error atom's region, truncating to fit.
    pub(crate) fn error_atom_set(&mut self, atom: Obj, text: &str) {
        let at = self.data(atom, 0).as_addr();
        let bytes = text.as_bytes();
        let n = bytes.len().min(ERROR_SCRATCH);
        for (i, &b) in bytes[..n].iter().enumerate() {
            self.heap.set_byte(at.plus(i as u64), b);
        }
        self.heap.set_byte(at.plus(n as u64), 0);
    }

    pub fn handle(&mut self, name: &str) -> Obj {
        let at = self.heap.store_bytes(name.as_bytes());
        let o = self.alloc(crate::objects::FLAG_HANDLE, 1);
        self.set_data(o, 0, Word(at.raw()));
        o
    }

    pub fn is_handle(&self, o: Obj) -> bool {
        self.is(o, crate::objects::FLAG_HANDLE)
    }

    pub fn sym(&mut self, name: &str) -> Obj {
        // Instruction names first: they are the same object in every base, so a
        // per-base intern must never mint a second one.
        if let Some(o) = self.shared_symbols.get(name) {
            return o;
        }
        if let Some(o) = self.symbols.get(name) {
            return o;
        }
        let at = self.heap.store_bytes(name.as_bytes());
        let o = self.alloc(FLAG_SYM, 1);
        self.set_data(o, 0, Word(at.raw()));
        self.symbols.put(name, o);
        o
    }

    /// Intern into the SHARED table — instruction names and the engine's own
    /// internal symbols, which must not differ between bases.
    pub fn sym_shared(&mut self, name: &str) -> Obj {
        if let Some(o) = self.shared_symbols.get(name) {
            return o;
        }
        let at = self.heap.store_bytes(name.as_bytes());
        let o = self.alloc(FLAG_SYM, 1);
        self.set_data(o, 0, Word(at.raw()));
        self.shared_symbols.put(name, o);
        o
    }

    pub fn sym_name(&self, o: Obj) -> String {
        self.str_val(o)
    }

    pub fn is_sym(&self, o: Obj) -> bool {
        self.is(o, FLAG_SYM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Interning is a CONTRACT requirement, not an optimisation: `eq?` on two
    /// spellings of one name must hold, so identity is pointer identity.
    #[test]
    fn one_object_per_spelling() {
        let mut o = Objects::new();
        assert_eq!(o.sym("alpha"), o.sym("alpha"));
        assert_ne!(o.sym("alpha"), o.sym("beta"));
    }

    /// Strings are NOT interned: two identical literals are two objects, which
    /// is why `(eq? "a" "a")` is false where `(eq? 'a 'a)` is true.
    #[test]
    fn strings_are_not_interned() {
        let mut o = Objects::new();
        assert_ne!(o.str_new("a"), o.str_new("a"));
    }

    #[test]
    fn a_string_round_trips_through_storage() {
        let mut o = Objects::new();
        let s = o.str_new("hello");
        assert_eq!(o.str_val(s), "hello");
        assert_eq!(o.byte_len(s), 5);
    }

    #[test]
    fn an_empty_string_is_length_zero_not_nil() {
        let mut o = Objects::new();
        let s = o.str_new("");
        assert!(!s.is_nil());
        assert_eq!(o.byte_len(s), 0);
    }
}
