; tools/contract/obj-layout.x — canonical layout of every object's header words.
;
; THIS ENGINE'S OWN DESCRIPTOR.  x-lang is reflective: lib/x/boot/reflect.x reads
; object header words at committed offsets, and under decision L1 those offsets
; come from the ENGINE, not from the language.  x-lang includes whichever engine
; it is paired with, so these values are the contract this implementation must
; keep — and they are deliberately NOT the C engine's.
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
(def %obj-units-heap 0)   ; NO collector chain -- see above
(def %obj-units-type 1)   ; pointer to the type object (nil for none)
(def %obj-units-flags 1)  ; the flags bitfield, held as an integer
(def %obj-slot-heap 0)    ; unused; kept so the descriptor's shape matches
(def %obj-slot-type 0)
(def %obj-slot-flags 1)
(def %obj-meta-len 2)     ; header length = the word where data begins

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
