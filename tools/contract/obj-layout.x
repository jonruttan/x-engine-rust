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
; THE ARENA.  Objects live in one pinned array of machine words, and an object
; pointer is the real machine address of its first word, as the reference's is:
; compiled x code follows the words stored in objects, so they have to be
; addresses.
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

; --- data layouts (words, relative to data start) ---
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
(def %obj-flag-own 32)          ; 0x20  object owns its storage
(def %obj-flag-ro 64)           ; 0x40  read-only
(def %obj-flag-meta 128)        ; 0x80  extended meta units prepended
