; tools/contract/base-paths.x -- committed routes through this engine's base.
;
; THIS ENGINE'S OWN DESCRIPTOR, under decision L1.  x-lang reaches into the base
; by walking these routes -- lib/x/boot/registry.x resolves them BY NAME at
; runtime -- and the routes are the ENGINE's to declare, not the language's.
;
; A row is (name base step ...) where each step is `f` (first) or `r` (rest),
; applied left to right.  A route ends at the CELL whose first is the value, not
; at the value: that extra `first` is the caller's, and the conformance prelude
; documents the same convention from the other side.
;
; THE STEPS ARE OURS, THE NAMES ARE NOT.  Decision L1 exists so that a different
; object model can arrange its base differently -- the C engine reaches its
; catalog through eleven steps where this reaches it in none -- but both must
; agree on what a route is CALLED, because the library asks by name.  x-lang's
; `make check-base-routes` derives the required set from the library's own call
; sites and holds this file to it.
;
; A FLAT SPINE, one cell per route, rather than the C's nested groups.  There is
; nothing here to group yet: this base carries eight things where the C's carries
; a hundred, and inventing a hierarchy for eight would be arranging furniture in
; an empty room.  It grows a shape when it grows contents.
;
; EVERY CELL EXISTS even when its value is nil.  A route that resolved to nothing
; would be indistinguishable from a route the engine forgot, and the library's
; walk would answer nil rather than failing.
;
; FORMAT (rigid, one entry per line -- the awk parses the same bytes):
;   (<name> base <step> ...)

(def %base-paths (lit (
  (prims base)                ; the primitive catalog, ((ns . ((method . prim) ...)) ...)
  (type-alist base r)         ; registered types, by name
  (error-str base r r)        ; the last error's text
  (err-line base r r r)
  (err-file base r r r r)
  (file-registry base r r r r r)
  (obj-meta-extra base r r r r r r)
  (env base r r r r r r r)    ; this base's environment, as a first-class value

  ; --- the heap's registration lists, and the allocation ceiling ---
  ; LISTS the engine prepends to, not collector internals: a registered callable
  ; is invoked by the CONSUMING LAYER, once per mark phase.  The engine records.
  (heap-mark-hooks base r r r r r r r r)
  (heap-free-hooks base r r r r r r r r r)
  (heap-mark-roots base r r r r r r r r r r)
  (alloc-limit base r r r r r r r r r r r)
  (alloc-count base r r r r r r r r r r r r)

  ; --- routes rooted at a TYPE OBJECT, not at the base ---
  ; A type is a spine for the same reason the base is: the library walks it by
  ; name.  Every cell exists; most are nil until the library fills them.
  (type-name type)
  (type-cvt type r)
  (type-display type r r)
  (type-display-stack type r r r)
  (type-io type r r r r)
  (type-iter type r r r r r)
  (type-proc type r r r r r r)
  (type-write type r r r r r r r)
  (type-write-stack type r r r r r r r r)
)))
