#!/bin/sh
# isa.sh -- every row of isa.x is actually reachable in the built engine.
#
# THE MANIFEST AND THE CODE ARE TWO PLACES. tools/contract/isa.x is what x-lang
# reads to decide what this engine can do; src/prims/*.rs is what it can
# actually do. Nothing kept them in step, so a row could be declared and
# missing, or renamed on one side only, and every gate would stay green -- the
# contract checks compare isa.x against x-lang's vocabulary, never against the
# binary.
#
# x-engine-c has had this check since before the split. This engine did not.
#
# It asks the ENGINE, not the source. Parsing Rust means knowing every
# constructor shape, and the first version of this check missed `int2` and
# reported thirty false differences. The binary cannot misreport what it
# registered.
set -e

cd "$(dirname "$0")/../.."
ENGINE=${X_BIN:-./target/release/x-engine}
[ -x "$ENGINE" ] || { echo "isa: no engine at $ENGINE (cargo build --release)" >&2; exit 2; }

W="${TMPDIR:-/tmp}/isa-check.$$"; mkdir -p "$W"; trap 'rm -rf "$W"' EXIT INT TERM

# The catalog rows `(ns method tag)` and the bare rows `(name tag)`, in the
# sections isa.x separates them into.
awk '
  /%isa-catalog/ { sec="cat"; next }
  /%isa-bare/    { sec="bare"; next }
  /%isa-values/  { sec="val"; next }
  /%isa-keep|%isa-aliases/ { sec="skip"; next }
  sec=="cat" || sec=="bare" || sec=="val" {
    line=$0; sub(/;.*/, "", line)
    if (match(line, /\(([^)]*)\)/)) {
      inner=substr(line, RSTART+1, RLENGTH-2)
      n=split(inner, w, /[ \t]+/)
      if (n>=1 && w[1] != "") print sec, w[1], (n>=2 ? w[2] : "")
    }
  }
' tools/contract/isa.x > "$W/rows"

# One probe per row, through the engine's own committed base paths.
{
  echo '(include "tools/contract/base-paths.x")'
  cat <<'X'
(def %assoc (fn (self k l)
  (match ((eq? l ()) ()) ((eq? (first (first l)) k) (first l)) (#t (self k (rest l))))))
(def %walk (fn (self steps o)
  (match ((eq? steps ()) o)
         ((eq? (first steps) (lit f)) (self (rest steps) (first o)))
         (#t (self (rest steps) (rest o))))))
(def %cat (first (%walk (rest (rest (%assoc (lit prims) %base-paths))) (%base))))
(def %coord (fn (_ ns me)
  (def d (%assoc ns %cat))
  (match ((eq? d ()) ()) (#t (%assoc me (rest d))))))
(def %miss 0)
(def %note (fn (_ ok name) (match (ok ()) (#t (%seq (set! %miss (+ %miss 1)) (error name))))))
X
  while read -r sec a b; do
    case "$sec" in
      cat)  printf '(match ((eq? (%%coord (lit %s) (lit %s)) ()) (error "MISSING %s/%s")) (#t ()))\n' "$a" "$b" "$a" "$b" ;;
      bare|val) printf '(guard (_ (error "MISSING %s")) %s)\n' "$a" "$a" ;;
    esac
  done < "$W/rows"
  echo '(error "isa-ok")'
} > "$W/probe.x"

out=$("$ENGINE" < "$W/probe.x" 2>&1 | tail -1)
rows=$(wc -l < "$W/rows" | tr -d ' ')
case "$out" in
  *isa-ok*) echo "ISA check: all $rows rows of isa.x resolve in the engine." ;;
  *) echo "ISA check FAILED: $out" >&2; exit 1 ;;
esac
