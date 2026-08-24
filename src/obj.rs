//! The four things a machine word can mean here, as four types.
//!
//! Everything in this engine used to be `u64`. An object reference, a byte
//! offset, a raw word read out of storage, a header bitfield, an environment
//! index and a plain integer were one type to the compiler, so passing an offset
//! where an object belonged was a silent success. `fn as_ptr(&self, o: Obj) ->
//! u64` was the confession: the difference existed only in the author's head.
//!
//! These are newtypes, so they cost nothing at runtime and every crossing
//! between them has to be written down. Where a crossing is genuinely
//! meaningful — an object IS its own address in this objects — it gets a named
//! method and a reason, rather than being invisible.

/// Bytes per machine word, and per fixnum: the `int/ptr-same-width` guarantee.
/// This is not a knob. The whole objects design assumes an object reference and an
/// integer are the same width.
pub const WORD: usize = 8;

/// An object reference.
///
/// It happens to be a byte offset into the objects, but that is a fact about this
/// engine's representation, not about what an `Obj` is. Code that wants the
/// offset says `addr()` and means it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Obj(u64);

/// Nil. x-lang's `()` is nil is NULL — one value, and the model is settled.
pub const NIL: Obj = Obj(0);

impl Obj {
    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }

    /// The address this object begins at.
    ///
    /// A re-tagging, not a conversion: an object IS its own offset here, which
    /// is exactly what makes `obj ->ptr` and `ptr ->obj` round-trip by
    /// construction. It works only because the `core` profile has no foreign
    /// door — no `dlopen`, no `ptr call` — so an offset never escapes to C.
    pub const fn addr(self) -> Addr {
        Addr(self.0)
    }

    /// Stored form, for writing an object reference into a data slot.
    pub const fn word(self) -> Word {
        Word(self.0)
    }
}

/// A byte offset into the objects.
///
/// Distinct from `Obj` because most addresses are NOT objects: the bytes of a
/// string, a raw region from `mem alloc`, a header slot part-way into an object.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Addr(u64);

impl Addr {
    pub const fn new(raw: u64) -> Self {
        Addr(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    /// The object beginning at this address. Only meaningful for an address that
    /// really is an object's start — `ptr ->obj` is the instruction that asserts
    /// it, and x-lang's contract calls that one UNCHECKED.
    pub const fn as_obj(self) -> Obj {
        Obj(self.0)
    }

    /// Move by a SIGNED byte count, wrapping.
    ///
    /// Signed because `lib/x/boot/reflect.x` reads prepended meta units at
    /// NEGATIVE offsets — below the object it started from. Wrapping because a
    /// program may compute a nonsense offset, and an engine that panicked would
    /// abort instead of reporting, which is the one failure a conformance suite
    /// cannot diagnose.
    pub const fn offset(self, bytes: i64) -> Addr {
        Addr(self.0.wrapping_add(bytes as u64))
    }

    /// Move forward by an unsigned byte count.
    pub const fn plus(self, bytes: u64) -> Addr {
        Addr(self.0.wrapping_add(bytes))
    }

    /// Index of the word this address falls in.
    pub const fn word_index(self) -> usize {
        (self.0 / WORD as u64) as usize
    }

    /// Which byte within that word.
    pub const fn byte_in_word(self) -> u32 {
        (self.0 % WORD as u64) as u32
    }

    /// The start of the word this address falls in.
    pub const fn word_base(self) -> Addr {
        Addr(self.0 & !(WORD as u64 - 1))
    }
}

/// A raw machine word, as read out of objects storage.
///
/// Deliberately inert: it has no meaning until a caller says what it is. That is
/// the point — `data(o, 0)` cannot know whether it holds an object, an integer
/// or an address, so it answers a `Word` and the typed accessor above it decides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Word(pub u64);

impl Word {
    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn as_obj(self) -> Obj {
        Obj(self.0)
    }

    pub const fn as_addr(self) -> Addr {
        Addr(self.0)
    }

    /// Two's-complement reinterpretation. x-lang's fixnums are machine integers,
    /// so this is a reading of the same bits rather than a conversion.
    pub const fn as_i64(self) -> i64 {
        self.0 as i64
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub const fn from_i64(v: i64) -> Word {
        Word(v as u64)
    }

    pub const fn from_usize(v: usize) -> Word {
        Word(v as u64)
    }
}

/// An object header's flag bitfield.
///
/// A type of its own so that `is(o, flags)` cannot be handed an integer that
/// merely happens to be in range — the flag constants and the header word are
/// now the same type, and nothing else is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Flags(u64);

impl Flags {
    pub const fn new(raw: u64) -> Self {
        Flags(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn from_word(w: Word) -> Self {
        Flags(w.0)
    }
}

/// An environment's identity.
///
/// There is no privileged "global" id. Since `base make` exists, the engine's
/// own context is simply the FIRST base, and a constant naming frame zero would
/// have made the host an implicit parent of every sandbox.
///
/// Environments live outside the objects in a plain Vec, so this is an index — but
/// a closure stores one in a data word, which is precisely where it could be
/// confused with an object reference. It could not be more different: applying a
/// closure whose captured environment was really an object would bind names into
/// arbitrary storage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EnvId(Obj);

impl EnvId {
    pub const fn from_obj(o: Obj) -> Self {
        EnvId(o)
    }

    pub const fn obj(self) -> Obj {
        self.0
    }

    pub const fn word(self) -> Word {
        self.0.word()
    }

    pub const fn from_word(w: Word) -> Self {
        EnvId(w.as_obj())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reflective read that needs signed offsets: reflect.x addresses meta
    /// units BELOW the object. An unsigned-only offset would have to be spelled
    /// as a huge positive number and the intent would vanish.
    #[test]
    fn a_negative_offset_addresses_backwards() {
        let a = Addr::new(64);
        assert_eq!(a.offset(-8), Addr::new(56));
        assert_eq!(a.offset(8), Addr::new(72));
    }

    /// A nonsense offset must wrap, not panic. An aborting engine reports
    /// nothing at all.
    #[test]
    fn offsetting_below_zero_wraps_rather_than_panicking() {
        let _ = Addr::new(0).offset(-8);
        let _ = Addr::new(u64::MAX).plus(16);
    }

    #[test]
    fn word_addressing_splits_into_index_and_byte() {
        let a = Addr::new(19);
        assert_eq!(a.word_index(), 2);
        assert_eq!(a.byte_in_word(), 3);
        assert_eq!(a.word_base(), Addr::new(16));
    }

    /// An object is its own address, and the round trip is identity. This is the
    /// property decision L1 rests on, stated where the conversion lives.
    #[test]
    fn an_object_round_trips_through_its_address() {
        let o = Obj(1234 * WORD as u64);
        assert_eq!(o.addr().as_obj(), o);
    }

    #[test]
    fn nil_is_zero_and_knows_it() {
        assert!(NIL.is_nil());
        assert!(!Obj(8).is_nil());
    }

    /// Fixnums are machine integers: the bits are reinterpreted, not converted,
    /// so a negative value survives a round trip through storage.
    #[test]
    fn a_negative_integer_survives_the_word_round_trip() {
        assert_eq!(Word::from_i64(-42).as_i64(), -42);
        assert_eq!(Word::from_i64(i64::MIN).as_i64(), i64::MIN);
    }
}
