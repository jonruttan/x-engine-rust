; tools/contract/isa.x -- the instruction set: every function reachable from x-lang.
;
; THIS ENGINE'S SURFACE, and it grows as the engine earns it.  x-lang's
; tools/check/engine-contract.sh maps every row here to a capability group and
; answers the resolver's question -- can this engine run x-lang -- so a row that
; is listed but not implemented is a lie the whole apparatus is built to catch.
;
; Fifteen rows, and no capability yet.  Every group here is partly built: the
; spine has no op, set!, eval or call/cc; raw-op has four of thirty-two.  A
; capability is COVERAGE of a group, so the declaration correctly claims none of
; them, and the resolver's refusal is still the specification.
;
; That is what makes this file a ratchet rather than a plan: a row appears when
; the engine earns it, and x-lang's apparatus -- not this file -- decides whether
; it did.  Tags are the reference engine's for each name, looked up rather than
; chosen, because a tag picked by the engine being judged would put it in charge
; of the group it lands in.
;
; FORMAT (rigid, one entry per line -- x-lang's awk parses the same bytes):
;   %isa-catalog: (ns method tag)   filed in the prims catalog
;   %isa-bare:    (name tag)        bound bare, no catalog entry
;   %isa-values:  (name)            non-prim VALUES bound by the engine
;
; Tags justify why an entry is native, and x-lang's vocabulary groups by them:
;   spine  alloc  raw-op  raw-mem  tok  io  ffi  sys  gc  types  hot

(def %isa-catalog (lit (
  ; --- bases: another interpreter context, not another environment ---
  ; A child base is born ROOTLESS, carrying the instruction set and nothing of
  ; the host's.  That is x-lang's isolation story; `bind` is the door a host
  ; hands capabilities through, one name at a time.
  (base make spine)
  (base eval spine)
  (base bind spine)
  (base def-global spine)     ; the depth-blind define an operative surface needs (x-lang#527)
  (base make-tok spine)       ; a base with NO types -- for the reader protocol
  (base make-type spine)      ; register a reader type: analyse + read handlers
  (ctrl call/cc spine)

  ; --- the heap ---
  ; THIS ENGINE HAS NO COLLECTOR, and implementing the group does not add one.
  ; `heap collect` must PRESERVE a reachable object and be idempotent from the
  ; caller's view; a heap that never frees satisfies both by construction rather
  ; than by sweeping carefully.  Registration is the engine's job and invocation
  ; the library's -- a mark hook is invoked once per mark phase BY THE CONSUMING
  ; LAYER, so the engine only puts it on a list the base addresses.
  (heap count gc)
  (heap collect gc)
  (heap mark gc)
  (heap sweep gc)
  (heap pin! gc)
  (heap mark-hook! gc)
  (heap free-hook! gc)
  (heap mark-root! gc)
  (heap check gc)             ; DEBUG: verify every reference slot on the chain
  (alloc limit! gc)           ; ENFORCED, not merely recorded

  ; --- the reader's tape and scorer ---
  ; An analyser is a state machine whose states are FUNCTIONS, and a token must
  ; be DELIMITED: the accept branch runs when a character arrives that the state
  ; rejects, so text ending mid-token is never scored.
  (buf make alloc)            ; a buffer VIEWING a string's bytes, non-owning
  (buf read tok)              ; the character the analyser is fed
  (buf tok tok)               ; the claimed text: retain mark to cursor
  (buf last-char tok)
  (buf retain tok)            ; advance past a finished token
  (buf reset tok)
  (buf append tok)
  (buf read-text tok)
  (tok read-str tok)          ; score every type per position; longest claim wins
  (tok read tok)

  ; --- machine operations, filed under int/char/obj ---
  ; Each is filed against the object its bare name is already bound to, so the
  ; coordinate and the bare binding cannot drift apart.  x-lang's conformance
  ; suite checks exactly that, and two separately-made primitives would agree by
  ; luck until one of them changed.
  (int + raw-op)
  (int - raw-op)
  (int * raw-op)
  (int / raw-op)
  (int % raw-op)
  (int & raw-op)
  (int | raw-op)
  (int ^ raw-op)
  (int ~ raw-op)
  (int << raw-op)
  (int >> raw-op)
  (int < raw-op)
  (int = raw-op)
  (int ->char raw-op)         ; the char door has no bare spelling either way
  (char ->int raw-op)
  (obj eq? raw-op)
  (obj same? raw-op)

  ; --- the heap door ---
  ; Tagged `ffi` because that is the tag the reference engine gives them, and a
  ; tag chosen by the engine being judged would decide its own group.  x-lang's
  ; vocabulary files them under reflect/ptr-casts, NOT under the foreign-call
  ; group: these are how a reflective language reads its own objects.
  ;
  ; A "pointer" here is a BYTE OFFSET into this engine's heap, never a machine
  ; address -- which is what keeps the two senses of `ffi` apart.  The foreign
  ; door below traffics in real addresses and is the ONLY place they appear; an
  ; offset arriving there, or an address arriving here, is a segfault rather than
  ; a wrong answer, and that is why `crate::foreign` keeps its own type for one.
  (obj ->ptr ffi)
  (ptr ->obj ffi)
  (ptr ->int ffi)
  (int ->ptr ffi)
  (str ->ptr ffi)
  (ptr ->str ffi)

  ; --- unchecked memory ---
  (ptr ref raw-mem)           ; (p off width) -> width bytes, little-endian
  (ptr set! raw-mem)          ; (p off value width)
  (ptr ref-word raw-mem)      ; byte offset, and it may be NEGATIVE
  (ptr set-word! raw-mem)
  (mem cmp raw-mem)
  (mem copy raw-mem)
  (mem set raw-mem)
  (str byte-sub raw-mem)      ; (s off LEN), not an end index

  ; --- allocation ---
  (str make alloc)
  (mem alloc alloc)
  (mem free alloc)            ; a NO-OP, honestly: the arena never reuses a region
  (str append alloc)
  (str ->sym alloc)           ; INTERNS -- the result is eq? to the same literal
  (sym ->str alloc)
  (bytes ->str alloc)
  (type make-instance alloc)
  (obj make alloc)            ; needs a REGISTERED type handle; nil otherwise
  (obj make-callable alloc)   ; a raw address dressed as callable; NOT callable
                              ;   here -- `core` has no foreign door to jump to

  ; --- the type registry ---
  (type make types)
  (iter make types)

  ; --- derived, but kept native for heat ---
  ; x-lang's own justification for this tag: each of these is expressible through
  ; reflection, and each sits in an inner loop.  They are native here for the same
  ; reason, not because the engine cannot express them.
  (type of hot)               ; stable per type: (type of 1) and (type of 2) agree
  (type ? hot)
  (str byte-len hot)
  (str byte-ref hot)          ; answers a CHAR; char/->int is the separate step
  (iter next hot)             ; MUTATES the iterator's state word
  (iter empty? hot)           ; PEEKS -- asking must not exhaust
  (iter step hot)             ; FUNCTIONAL: (value . next-ITERATOR), receiver untouched

  ; --- the process I/O boundary ---
  ; All four read or write the SAME stream the program arrived on.  The engine
  ; owns its reader for that reason: what read-char should answer is whatever is
  ; left after the form being evaluated, which a reader living outside the engine
  ; could not be asked.
  (io write-str io)           ; raw bytes to the current output; answers NIL, not a count
  (io read io)                ; one FORM, unevaluated
  (io read-char io)           ; one byte; nil at end of input
  (io repl-read io)           ; the same act -- prompting and echo are the library's

  ; --- the foreign door ---
  ; The engine's ONLY unsafe code, walled into `src/foreign.rs` behind a module
  ; that re-permits it; the crate denies unsafe everywhere else.  A door out of a
  ; safe language cannot itself be safe, so the honest thing is to make it one
  ; small named place rather than to pretend the property survives.
  ;
  ; Addresses crossing here are REAL.  Handing one a heap offset dereferences
  ; whatever sits at that address in the process, which is why an offset and an
  ; address are separate types on this side of the wall.
  (ffi dlopen ffi)            ; () for the process itself -- the handle x-lang asks for
  (ffi dlsym ffi)             ; NIL when unresolved, never a raise: absence is an answer
  (ffi call ffi)              ; VARIADIC; doubles cross as their bit patterns
  (ptr call ffi)              ; VARIADIC; integer and pointer arguments

  ; --- the OS ---
  (sys clock sys)             ; process CPU time in microseconds, and MONOTONIC
)))

(def %isa-bare (lit (
  ; --- the evaluator ---
  (error spine)               ; the only channel a bare engine has: raises, and a
                              ;   raise at top level ENDS the run -- checked
                              ;   against x-engine-c rather than assumed
  (lit spine)
  (def spine)
  (fn spine)                  ; applicative; binds its first parameter to itself
  (match spine)
  (guard spine)
  (%seq spine)                ; evaluate each, answer the last
  (wrap spine)                ; applicative over an operative; holds it, not a copy
  (unwrap spine)              ; the very same object back -- same?, not equal
  (atomic spine)
  (tail-eval spine)
  (call/cc spine)             ; ESCAPE-only; unwinds outward, cannot be re-entered
  (%cc-invoke spine)
  (op spine)                  ; operative: args UNEVALUATED, caller's env by name
  (set! spine)                ; rebinds where a name already lives; never creates
  (eval spine)
  (eval! spine)               ; eval in the CURRENT env
  (apply spine)
  (%base spine)               ; the reflective root; routes in base-paths.x

  ; --- machine operations ---
  (eq? raw-op)                ; numbers by value, everything else by identity
  (same? raw-op)              ; STRICT identity; what eq? cannot answer
  (+ raw-op)
  (- raw-op)
  (* raw-op)
  (/ raw-op)
  (% raw-op)
  (& raw-op)
  (| raw-op)
  (^ raw-op)
  (~ raw-op)
  (<< raw-op)
  (>> raw-op)
  (< raw-op)
  (= raw-op)

  ; --- memory ---
  (first raw-mem)
  (rest raw-mem)
  (pair alloc)

  ; --- i/o ---
  (include io)
  (alloc-limit! gc)           ; BARE as well as filed: a harness arms it before
                              ;   anything loads, and every runner does

  ; --- the kernel, and interrupts ---
  ; Bare because that is where the reference binds them and where the library
  ; reaches for them: lib/x/sys/posix.x is built on a bare `syscall`.
  (syscall ffi)               ; VARIADIC; the number is the platform's, not x-lang's
  (sigint-install sys)        ; SIGINT sets %sigint-flag instead of ending the run
  (sigint-restore sys)        ; the default disposition, so the process ends as it began
)))

(def %isa-keep (lit (
)))

(def %isa-aliases (lit (
  (%raw-include include)
)))

(def %isa-values (lit (
  ; A bound OBJECT, not an instruction.  The signal handler cannot write it --
  ; a handler may touch only async-signal-safe state, and this engine's heap
  ; never qualifies -- so the handler sets an atomic and the eval loop publishes
  ; it here, between forms.  That is soon enough for the one thing that reads it:
  ; a cancel is observed by the NEXT form, not inside the interrupted one.
  (%sigint-flag)

  ; The clean end-of-input sentinel, answered by `io repl-read`.  A UNIQUE
  ; object compared by IDENTITY -- lib/x/repl/loop.x uses (obj same?) and says
  ; why: "eq? compares value words and could conflate a satom with an integer".
  ;
  ; It is what gives a REPL three outcomes where a reader has two: a value (nil
  ; included, since `()` reads as nil), a clean end of input, and a truncated
  ; form, which arrives as a raise.  `io read` folds it to nil; `repl-read` does
  ; not, and that is the ONLY difference between them.
  (%token-eof)

  ; --- the truth values ---
  ; Bound from interned singletons, not from name literals: `#t` evaluates to
  ; the symbol itself and `#f` to the false object, and a form read in the host
  ; and evaluated in a CHILD base must find both.
  (#t)
  (#f)

  ; --- the invocation ---
  ; Every argv element, as a list of strings.  The engine parses NONE of it:
  ; `--batch` is a string here, and an engine with opinions about it would be
  ; implementing a protocol that belongs to the wrapper.
  (args)

  ; --- identity ---
  ; x-machine is the build TRIPLE, and it is read rather than decorative:
  ; lib/x/platform/syscall.x scans it for "darwin"/"linux" and for
  ; "arm64"/"aarch64"/"x86_64" to choose a syscall table.
  ;
  ; x-version is the EXPRESSION LAYER's version and stays stable; x-release is
  ; WHICH RELEASE of x-lang this engine is, stamped at build time.  They are two
  ; values on purpose: before the reference separated them, two releases whose
  ; sources never changed reported identically, and a pinned amalgam from one
  ; could boot on the other and segfault (x-lang #435).
  (x-machine)
  (x-release)
  (x-version)
)))
