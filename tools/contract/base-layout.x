; tools/contract/base-layout.x -- the base cells that hold language state.
;
; x-lang's image writer and reader (tools/dev/image-write.x, image-read.x)
; read this file as data: every (cell NAME) or (slot NAME) inside a
; (build ...) is a language root, and each NAME is resolved through
; tools/contract/base-paths.x.  The labels are x-engine-c's:
;   (node NAME KIDS...)  a named grouping, walked for its kids
;   (pair L R)           an anonymous grouping
;   (cell NAME)          a route that ends at a cell whose first is the value
;   (slot NAME)          a route that ends at the value itself
;   (build SUBTREE)      the cells the engine builds for each base
;   (todo NAME)          state that is the engine's, not the language's
;   (nil)                the end of a list
;
; THIS ENGINE'S BASE IS A FLAT SPINE (see base-paths.x).  A route that ends at
; a spine pair or a holder whose first is the value is a cell; env-root and
; save-stack reach their value itself, as the reference's slots of those
; names do.  The names are the ones x-engine-c's layout builds that this base
; also carries; token-cache, eval-list, file and parent are not carried, and
; the profile is one cell of the engine's own.

(node base
  (build
    (pair
      (node env
        (cell env)
        (slot env-root))
      (node ctrl
        (pair (slot save-stack)
              (cell error-handler))
        (pair (cell tco-expr)
              (cell tco-env)))))
  (todo profile))

(node io-fields
  (build (cell type-alist))
  (todo files)
  (build
    (node io-state
      (cell line)
      (pair (cell true)
            (cell false)))))

(node state
  (build
    (pair (cell sigint)
      (pair (cell err)
        (pair (cell prims)
          (pair (cell err-line)
            (pair (cell err-file)
                  (cell file-registry))))))))
