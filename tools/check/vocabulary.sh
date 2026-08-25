#!/bin/sh
# vocabulary.sh -- x-lang's NAMES live in src/vocabulary.rs and nowhere else.
#
# WHY THIS IS A GATE AND NOT A CONVENTION. The names had spread across twenty
# files as string literals: type names, handler families, character names, the
# ffi conventions, the words a diagnostic says. Nothing was wrong with any one
# of them. What was wrong is that changing the initial syntax, or emitting
# diagnostics in another language, meant reading every module to work out which
# literals were vocabulary and which were Rust -- and no reviewer catches the
# twenty-first.
#
# So it is checked. A vocabulary literal outside the module fails the build, and
# the fix is to add a name to src/vocabulary.rs rather than to this allowlist.
#
# Two things are deliberately NOT vocabulary:
#   * the PrimDef tables -- a coordinate belongs beside the function it names,
#     and tools/check/isa.sh proves those agree with the manifest;
#   * anything in tools/contract/vocabulary-allow.txt, each line carrying its
#     reason.
set -e

cd "$(dirname "$0")/../.."
ALLOW=tools/contract/vocabulary-allow.txt
W="${TMPDIR:-/tmp}/vocab.$$"; mkdir -p "$W"; trap 'rm -rf "$W"' EXIT INT TERM

# Literals that LOOK like x-lang: lowercase words, `%`-privates, `#`-forms.
# Rust's own strings are capitalised, punctuation, or format fragments.
#
# NOTHING FILTERS awk's OUTPUT HERE. A `grep -oE '^[^\t]*\t.*'` used to sit
# between the two, and it was worse than useless: awk already emits exactly
# `FILENAME <tab> line`, so it could only ever discard. BSD grep reads `\t` as a
# literal `t` rather than a tab, so on macOS it discarded most files' lines and
# this gate passed on a fraction of the source -- a vacuous pass that only showed
# up when Linux CI, with GNU grep, reported five literals macOS had never seen.
for f in $(find src -name '*.rs' | grep -v 'src/vocabulary.rs' | sort); do
	awk '
		/^[ \t]*\/\// { next }
		/#\[cfg\(test\)\]/ { intest=1 }
		intest { next }
		/PrimDef::/ { next }
		/crate::uniform_/ { next }
		{ print FILENAME "\t" $0 }
	' "$f"
done | while IFS="$(printf '\t')" read -r file line; do
	printf '%s' "$line" | grep -oE '"[^"]*"' | grep -E '^"[a-z#%]' | while read -r lit; do
		printf '%s\t%s\n' "$file" "$lit"
	done
done | sort -u > "$W/found"

# Drop the allowed ones, matched on the literal alone.
if [ -f "$ALLOW" ]; then
	awk -F'\t' '!/^#/ && NF { print $1 }' "$ALLOW" | sort -u > "$W/allow"
else
	: > "$W/allow"
fi
# THE SCAN MUST HAVE SEEN SOMETHING. An extraction that silently produces
# nothing passes this gate perfectly, which is how the non-portable grep above
# went unnoticed for as long as it did: a broken scan and a clean repo are
# indistinguishable from the exit status alone. This repo has x-lang literals
# outside src/vocabulary.rs by design -- base.rs alone carries the route names --
# so an empty scan is a broken scan, not a clean one.
if [ ! -s "$W/found" ]; then
	echo "vocabulary: the scan found NO literals at all in src/**.rs." >&2
	echo "  That is this gate failing, not passing -- the extraction is broken." >&2
	exit 1
fi

cut -f2 "$W/found" | sort -u > "$W/lits"
comm -23 "$W/lits" "$W/allow" > "$W/bad"

if [ -s "$W/bad" ]; then
	echo "vocabulary: x-lang names outside src/vocabulary.rs:" >&2
	while read -r lit; do
		grep -F "	$lit" "$W/found" | head -3 | sed 's/^/  /' >&2
	done < "$W/bad"
	echo "  Move the name to src/vocabulary.rs, or -- if it is not vocabulary --" >&2
	echo "  add it to $ALLOW with the reason." >&2
	exit 1
fi
echo "vocabulary: every x-lang name is in src/vocabulary.rs."
