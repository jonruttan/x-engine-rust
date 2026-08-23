//! The heap: word storage.
//!
//! NAMED FOR THE CONTRACT, not for the implementation. x-lang's ISA has a `heap`
//! namespace — `heap collect`, `heap count`, `heap mark`, `heap free-hook!` —
//! and the object header's word 0 is documented as the heap link word. This was
//! called `Store` for a while, which meant the contract and the code would have
//! used different words for the same thing the moment isa/gc arrived.
//!
//! The word arena only GROWS: freed objects go back on the collector's free list
//! (see `collect.rs`) and are handed out again, but nothing is ever returned to
//! the allocator and nothing is compacted. That is what `gc/non-moving`
//! promises — every address x-lang holds stays valid for the life of the run.
//!
//! A flat `Vec<u64>` and the operations on it: read a word, write a byte, copy a
//! block, allocate a run. It knows NOTHING about objects — no headers, no flags,
//! no types, no pairs. That is the whole point of it being its own file: every
//! operation here is checkable against a picture of memory and nothing else.
//!
//! Index 0 is reserved and never allocated, so offset 0 can mean nil.
//!
//! LITTLE-ENDIAN falls out rather than being decided: byte i of a word is its
//! i-th lowest, so a widening read assembles from the low end. On a big-endian
//! host the same code answers differently, which is why x-lang records `endian`
//! as a constraint rather than legislating it.

use crate::obj::{Addr, Word, WORD};

pub struct Heap {
    words: Vec<u64>,
    /// How many objects have been allocated.
    ///
    /// Allocations EVER, not live objects: this is `heap count`, which x-lang
    /// requires only to RISE across an allocation, so collection must not lower
    /// it. `Objects::live` is the other number.
    allocations: usize,
}

impl Heap {
    pub fn new() -> Self {
        // One reserved word so no real allocation can land at offset 0.
        Heap {
            words: vec![0],
            allocations: 0,
        }
    }

    /// The address the next allocation will begin at.
    pub fn frontier(&self) -> Addr {
        Addr::new((self.words.len() * WORD) as u64)
    }

    /// An out-of-range read answers zero rather than panicking. x-lang computes
    /// these offsets itself, a program can compute a wrong one, and an engine
    /// that aborted would report nothing at all — the one failure a conformance
    /// suite cannot diagnose.
    pub fn word(&self, at: Addr) -> Word {
        Word(self.words.get(at.word_index()).copied().unwrap_or(0))
    }

    /// An out-of-range write is dropped, for the same reason.
    pub fn set_word(&mut self, at: Addr, v: Word) {
        let i = at.word_index();
        if i < self.words.len() {
            self.words[i] = v.raw();
        }
    }

    pub fn byte(&self, at: Addr) -> u8 {
        let w = self.word(at.word_base()).raw();
        ((w >> (8 * at.byte_in_word())) & 0xff) as u8
    }

    pub fn set_byte(&mut self, at: Addr, v: u8) {
        let base = at.word_base();
        let sh = 8 * at.byte_in_word();
        let mut w = self.word(base).raw();
        w &= !(0xffu64 << sh);
        w |= (v as u64) << sh;
        self.set_word(base, Word(w));
    }

    /// Append one word, growing the heap.
    /// How many objects have been allocated.
    ///
    /// Allocations EVER — see the field. `Objects::live` is what is on the chain.
    pub fn allocations(&self) -> usize {
        self.allocations
    }

    pub fn note_allocation(&mut self) {
        self.allocations += 1;
    }

    /// The REAL machine address of a heap offset.
    ///
    /// Safe to compute and NOT safe to dereference, which is why it lives behind
    /// a name: `crate::foreign` hands one to C for the duration of a call, and
    /// that is sound only because nothing re-enters the engine during it, so this
    /// Vec cannot reallocate under the callee.
    ///
    /// Nothing else has business calling this. An offset is what the object model
    /// uses; an address is what leaves.
    pub fn address_of(&self, at: Addr) -> u64 {
        self.words.as_ptr() as u64 + at.raw()
    }

    pub fn words_len(&self) -> usize {
        self.words.len()
    }

    pub fn push(&mut self, v: Word) {
        self.words.push(v.raw());
    }

    /// A run of `n` zeroed bytes. Never freed: the whole memory manager here is
    /// that the heap grows, which the `core` profile permits precisely because
    /// it has no isa/gc.
    pub fn alloc_bytes(&mut self, n: usize) -> Addr {
        let start = self.frontier();
        for _ in 0..n.div_ceil(WORD).max(1) {
            self.words.push(0);
        }
        start
    }

    /// `width` bytes assembled into the low end of a zeroed integer, and its
    /// inverse.
    ///
    /// These are a PAIR and live together because they state one rule twice.
    /// Split across two call sites, changing one and not the other breaks the
    /// round trip while every individual test still passes.
    pub fn read_le(&self, at: Addr, width: u32) -> u64 {
        let mut v: u64 = 0;
        for i in 0..width as u64 {
            v |= (self.byte(at.plus(i)) as u64) << (8 * i);
        }
        v
    }

    pub fn write_le(&mut self, at: Addr, v: u64, width: u32) {
        for i in 0..width as u64 {
            self.set_byte(at.plus(i), ((v >> (8 * i)) & 0xff) as u8);
        }
    }

    pub fn copy_bytes(&mut self, dst: Addr, src: Addr, n: u64) {
        for i in 0..n {
            let v = self.byte(src.plus(i));
            self.set_byte(dst.plus(i), v);
        }
    }

    pub fn fill_bytes(&mut self, dst: Addr, v: u8, n: u64) {
        for i in 0..n {
            self.set_byte(dst.plus(i), v);
        }
    }

    /// Heap bytes NUL-terminated and answer the address of the first.
    ///
    /// x-lang rules that str values ARE C strings — bytes past the NUL are
    /// unobservable — and storing them terminated is what makes that true here
    /// rather than something to remember at each site.
    pub fn store_bytes(&mut self, bytes: &[u8]) -> Addr {
        let start = self.frontier();
        let mut w: u64 = 0;
        let mut n = 0;
        for &c in bytes.iter().chain(std::iter::once(&0u8)) {
            w |= (c as u64) << (8 * n);
            n += 1;
            if n == WORD {
                self.words.push(w);
                w = 0;
                n = 0;
            }
        }
        if n > 0 {
            self.words.push(w);
        }
        start
    }

    /// Read a NUL-terminated run back out.
    pub fn bytes_at(&self, at: Addr) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0u64;
        loop {
            let c = self.byte(at.plus(i));
            if c == 0 {
                break;
            }
            out.push(c);
            i += 1;
        }
        out
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test here runs against a picture of memory. No objects, no engine —
    /// which is what having storage as its own part buys.
    #[test]
    fn nothing_is_allocated_at_offset_zero() {
        let mut s = Heap::new();
        assert!(s.alloc_bytes(8).raw() > 0);
    }

    #[test]
    fn a_byte_written_reads_back_at_its_own_offset() {
        let mut s = Heap::new();
        let at = s.alloc_bytes(32);
        s.set_byte(at.plus(3), 200);
        s.set_byte(at.plus(4), 7);
        assert_eq!(s.byte(at.plus(3)), 200);
        assert_eq!(s.byte(at.plus(4)), 7);
    }

    /// The property read_le and write_le must JOINTLY satisfy. The failure they
    /// guard against is the two disagreeing, which neither can catch alone.
    #[test]
    fn a_widening_write_reads_back_at_every_width() {
        let mut s = Heap::new();
        let at = s.alloc_bytes(16);
        for width in 1..=8u32 {
            let v = 0x0102_0304_0506_0708u64 >> (8 * (8 - width));
            s.write_le(at, v, width);
            assert_eq!(s.read_le(at, width), v, "width {}", width);
        }
    }

    #[test]
    fn a_widening_read_takes_the_low_byte_first() {
        let mut s = Heap::new();
        let at = s.alloc_bytes(8);
        s.set_byte(at, 1);
        s.set_byte(at.plus(1), 0);
        assert_eq!(s.read_le(at, 4), 1, "not 16777216");
    }

    #[test]
    fn blocks_copy_and_fill() {
        let mut s = Heap::new();
        let a = s.alloc_bytes(8);
        let b = s.alloc_bytes(8);
        s.fill_bytes(a, 65, 8);
        s.copy_bytes(b, a, 8);
        for i in 0..8 {
            assert_eq!(s.byte(b.plus(i)), 65);
        }
    }

    /// The C-string ruling, at the level it actually holds: bytes past the NUL
    /// are unobservable.
    #[test]
    fn stored_bytes_are_nul_terminated() {
        let mut s = Heap::new();
        let at = s.store_bytes(b"abc");
        assert_eq!(s.bytes_at(at), b"abc");
        s.set_byte(at.plus(1), 0);
        assert_eq!(s.bytes_at(at), b"a");
    }

    #[test]
    fn an_out_of_range_access_is_zero_not_a_panic() {
        let mut s = Heap::new();
        let far = Addr::new(1 << 30);
        assert_eq!(s.word(far), Word(0));
        assert_eq!(s.byte(far), 0);
        s.set_word(far, Word(5));
        assert_eq!(s.word(far), Word(0), "the write is dropped");
    }
}
