; tools/contract/obj-layout.x — canonical layout of every object's header words.
;
; THIS ENGINE'S OWN DESCRIPTOR.  x-lang is reflective: lib/x/boot/reflect.x reads
; object header words at committed offsets, and under decision L1 those offsets
; come from the ENGINE, not from the language.  x-lang includes whichever engine
; it is paired with, so these values are the contract this implementation must
; keep.
;
; Heap link at 0, type at 1, flags at 2, LENGTH at 3, data at 4.
;
; The first three are the C engine's. The fourth is this engine's own, and the
; deviation is deliberate: a sweep has to know how far an object extends, and
; the reference encodes that in its flags word. x-lang READS flags — it masks
; them with %obj-flag-attr-mask — so packing a length in beside them would put
; the collector's bookkeeping inside a value the language inspects. A word of
; its own costs 8 bytes an object and keeps the two apart.
;
; That is what decision L1 is for: the STEPS are the engine's, the NAMES are the
; contract, and a consumer reads %obj-meta-len rather than assuming three. The header was two
; words while the engine had no collector: with nothing to sweep there was no
; chain to thread, so the slot was reserved and left out of the count.
;
; Giving the engine a collector makes the slot real. Nothing in the library
; needed editing for it: `%data-off-0` is computed from `%obj-meta-len`, so the
; offsets follow from here. That is decision L1 working -- the layout travels
; with the engine, and the one place it is written down is this file.
;
; Units are words; multiply by %word-size for byte offsets.
;
; NO HEAP LINK.  x-engine-c reserves word 0 for a garbage-collector chain.  This
; engine has no collector: the `core` profile does not include isa/gc, and the
; smallest thing that can boot x-lang therefore allocates and never frees.  So the
; link word is absent, every later slot shifts down by one, and the header is two
; words rather than three.  The C engine's own descriptor documents this exact
; shape for its non-X_HEAP build, which is what makes it a legitimate variation
; rather than an invention.
;
; That difference is the point.  If x-lang boots on both, L1 is doing its job; if
; it only boots on a layout identical to the C one, "engine-supplied layout" was
; never real.
;
; THE ARENA.  Objects live in a flat array of machine words.  An object pointer is
; a BYTE OFFSET into that array, not a machine address -- nothing outside this
; engine ever dereferences one, because the `core` profile has no foreign door.
; Every reflective read x-lang performs lands back inside the arena by
; construction.
;
; FORMAT (rigid, one entry per line -- the awk parses the same bytes):
;   (def %obj-<name> <decimal integer>)

; --- header slots (words, relative to the object pointer) ---
(def %obj-units-heap 1)   ; the collector's chain link
(def %obj-units-type 1)   ; pointer to the type object (nil for none)
(def %obj-units-flags 1)  ; the flags bitfield, held as an integer
(def %obj-units-len 1)    ; how many DATA words follow the header
(def %obj-slot-heap 0)    ; every object is threaded here when allocated
(def %obj-slot-type 1)
(def %obj-slot-flags 2)
(def %obj-slot-len 3)
(def %obj-meta-len 4)     ; header length = the word where data begins

; --- data shapes (words, relative to data start) ---
(def %obj-units-atom 1)   ; atom: the value word (int / str offset / char)
(def %obj-units-pair 2)   ; pair: first at data 0, rest at data 1
(def %obj-slot-first 0)
(def %obj-slot-rest 1)

; --- flags word bits (decimal; hex noted in comments) ---
; Deliberately the same VALUES as x-engine-c uses.  The descriptor is this
; engine's to choose, but x-lang reads these bits by name from whichever engine
; it booted, and matching a known-good assignment removes a whole class of
; difference that would prove nothing.  The layout above differs where there is a
; reason; this does not, because there is none.
(def %obj-flag-attr-mask 15)    ; 0x0F
(def %obj-flag-1 1)             ; 0x01  WRAP / SHADOW
(def %obj-flag-2 2)             ; 0x02  COV
(def %obj-flag-3 4)             ; 0x04
(def %obj-flag-4 8)             ; 0x08
(def %obj-flag-simple-type 16)  ; 0x10  marker bit: a simple-type code follows
(def %obj-flag-prim 16)         ; 0x10
(def %obj-flag-fn 17)           ; 0x11
(def %obj-flag-int 18)          ; 0x12
(def %obj-flag-char 19)         ; 0x13
(def %obj-flag-str 20)          ; 0x14
(def %obj-flag-ptr 21)          ; 0x15
(def %obj-flag-type-mask 240)   ; 0xF0
(def %obj-flag-own 32)          ; 0x20  object owns its storage
(def %obj-flag-ro 64)           ; 0x40  read-only
(def %obj-flag-meta 128)        ; 0x80  extended meta units prepended
