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
for f in $(find src -name '*.rs' | grep -v 'src/vocabulary.rs' | sort); do
	awk '
		/^[ \t]*\/\// { next }
		/#\[cfg\(test\)\]/ { intest=1 }
		intest { next }
		/PrimDef::/ { next }
		{ print FILENAME "\t" $0 }
	' "$f"
done | grep -oE '^[^\t]*\t.*' | while IFS="$(printf '\t')" read -r file line; do
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
