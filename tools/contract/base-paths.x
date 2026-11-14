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
; catalogue through eleven steps where this reaches it in none -- but both must
; agree on what a route is CALLED, because the library asks by name.  x-lang's
; `make check-base-routes` derives the required set from the library's own call
; sites and holds this file to it.
;
; THE BASE IS TWO UNITS, as the reference's is: the spine first, the slot
; vector second (tools/contract/base-slots.x).  So every row's first step is
; `f`, into the spine.
;
; A FLAT SPINE, one cell per route, rather than the C's nested fields.  There is
; nothing here to nest yet: this base carries eight things where the C's carries
; a hundred, and inventing a hierarchy for eight would be arranging furniture in
; an empty room.  It grows a layout when it grows contents.
;
; EVERY CELL EXISTS even when its value is nil.  A route that resolved to nothing
; would be indistinguishable from a route the engine forgot, and the library's
; walk would answer nil rather than failing.
;
; FORMAT (rigid, one entry per line -- the awk parses the same bytes):
;   (<name> base <step> ...)

(def %base-paths (lit (
  (prims base f)                ; the primitive catalogue, ((ns . ((method . prim) ...)) ...)
  (type-alist base f r)         ; registered types, by name
  (error-str base f r r)        ; the last error's text
  (err-line base f r r r)
  (err-file base f r r r r)
  (file-registry base f r r r r r)
  (obj-meta-extra base f r r r r r r)
  (env base f r r r r r r r)    ; this base's environment, as a first-class value

  ; --- the heap's registration lists, and the allocation ceiling ---
  ; LISTS the engine prepends to, not collector internals: a registered callable
  ; is invoked by the CONSUMING LAYER, once per mark phase.  The engine records.
  (heap-mark-hooks base f r r r r r r r r)
  (heap-free-hooks base f r r r r r r r r r)
  (heap-mark-roots base f r r r r r r r r r r)
  (alloc-limit base f r r r r r r r r r r r)
  (alloc-count base f r r r r r r r r r r r r)

  ; --- routes the LIBRARY walks by name ---
  ; Not engine state: cells x-lang reads and writes. They used to be reached by
  ; literal first/rest chains in lib/, which assumed x-engine-c's layout; the
  ; library now resolves them by name, which is what decision L1 requires.
  (line    base f r r r r r r r r r r r r r f)
  (files   base f r r r r r r r r r r r r r r f)
  (profile base f r r r r r r r r r r r r r r r)
  ; The false SINGLETON, whose REST x-lang uses as scratch (module.x hangs the
  ; include list there), so this answers the OBJECT ITSELF -- the final step
  ; matters: without it the walk ends on the spine node, and module.x's
  ; set-rest! CUTS THE SPINE, orphaning every row behind it.
  (false   base f r r r r r r r r r r r r r r r r f)
  ; The REPL's pair: the fd being read, and the buffer being read from.
  ; Rows that land on the value, as the reference's are: the walk lands ON the
  ; input stack,
  ; whose first is the current fd / read buffer.
  (filein  base f r r r r r r r r r r r r r r r r r f)
  (buffer  base f r r r r r r r r r r r r r r r r r r f)

  ; --- the evaluator's own state ---
  ; Rows that land on the value, as the reference's do for these names.  save-stack is nil at the top level and non-nil over a closure
  ; body's non-tail forms; the tco pair holds the deferred tail expression and
  ; its environment; sigint is the CELL holding the object %sigint-flag names
  ; (first reaches the flag, as on the reference); error-handler is the CELL
  ; whose first is the active guard chain, as the reference's handler cell is.
  (save-stack    base f r r r r r r r r r r r r r r r r r r r f)
  (tco-expr      base f r r r r r r r r r r r r r r r r r r r r f)
  (tco-env       base f r r r r r r r r r r r r r r r r r r r r r f)
  (sigint        base f r r r r r r r r r r r r r r r r r r r r r r)
  (error-handler base f r r r r r r r r r r r r r r r r r r r r r r r)
  ; The true SINGLETON, a row that lands on the value as false's is.
  (true          base f r r r r r r r r r r r r r r r r r r r r r r r r f)
  ; The root environment, (bindings . ()), whose bindings are a tree.
  (env-root base f r r r r r r r r r r r r r r r r r r r r r r r r r f)
  ; The raised-error value: one ERR, (code . subject), each a string atom a
  ; raise repoints.  x-lang reaches its type through it.
  (err base f r r r r r r r r r r r r r r r r r r r r r r r r r r f)

  ; --- routes rooted at a TYPE OBJECT, not at the base ---
  ; These steps are the REFERENCE ENGINE'S, chosen
  ; deliberately rather than invented.  Decision L1 leaves the steps to the
  ; engine -- only the names are the contract -- so a flat spine would have been
  ; permitted.  It would also have been a fresh set of decisions about a
  ; structure whose real ones are already paid for, and this engine has been
  ; wrong about a spine before by inventing one.
  ;
  ; The layout: eight top-level cells -- name, data, heap, proc, cvt, io, iter,
  ; ops -- each field holding one cell per family.  A family's cell holds a
  ; STACK (a list), so `type-X-stack` addresses the list and `type-X`, one `f`
  ; deeper, addresses its head: the ACTIVE handler.  The library pushes and pops
  ; by writing the PARENT of the stack route, which is what %reflect-path-parent
  ; in lib/x/boot/registry.x derives.
  ;
  ; Every cell exists from birth though nearly all are nil, for the same reason
  ; the base's do: a route that walks off the end answers nil, and the library
  ; cannot tell that from "no handler installed".
  (type-name-stack type f)
  (type-name type f f)
  (type-data-stack type r f)
  (type-data type r f f)
  (type-heap type r r f)
  (type-mark-stack type r r f f)
  (type-mark type r r f f f)
  (type-make-stack type r r f r f)
  (type-make type r r f r f f)
  (type-free-stack type r r f r r f)
  (type-free type r r f r r f f)
  (type-clone-stack type r r f r r r f)
  (type-clone type r r f r r r f f)
  (type-units-stack type r r f r r r r f)
  (type-units type r r f r r r r f f)
  (type-length-stack type r r f r r r r r f)
  (type-length type r r f r r r r r f f)
  (type-proc type r r r f)
  (type-call-stack type r r r f f)
  (type-call type r r r f f f)
  (type-eval-stack type r r r f r f)
  (type-eval type r r r f r f f)
  (type-cvt type r r r r f)
  (type-from-stack type r r r r f f)
  (type-from type r r r r f f f)
  (type-to-stack type r r r r f r f)
  (type-to type r r r r f r f f)
  (type-io type r r r r r f)
  (type-analyse-stack type r r r r r f f)
  (type-analyse type r r r r r f f f)
  (type-delimit-stack type r r r r r f r f)
  (type-delimit type r r r r r f r f f)
  (type-read-stack type r r r r r f r r f)
  (type-read type r r r r r f r r f f)
  (type-write-stack type r r r r r f r r r f)
  (type-write type r r r r r f r r r f f)
  (type-display-stack type r r r r r f r r r r f)
  (type-display type r r r r r f r r r r f f)
  (type-iter-fields type r r r r r r f)
  (type-iter-stack type r r r r r r f f)
  (type-iter type r r r r r r f f f)
  (type-ops-fields type r r r r r r r f)
  (type-ops-stack type r r r r r r r f f)
  (type-ops type r r r r r r r f f f)
)))
