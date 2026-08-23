#!/bin/sh
# check.sh -- everything that must hold before a commit.
#
# It exists because none of it was happening. Formatting, lint and unit tests
# were all things to remember, and what actually got remembered was the
# conformance score -- so the engine drifted a thousand lines away from
# rustfmt, collected zero tests, and grew a 615-line dispatcher, while the
# number it was measured by went up every day.
#
# A score is not a check. This is.
#
#   sh check.sh                  fmt, clippy, unit tests
#   sh check.sh /path/to/x-lang  ...and the conformance suite as well
set -e

cd "$(dirname "$0")"
XLANG="${1:-}"

# --- find a toolchain --------------------------------------------------------
# `cargo` on PATH is the normal case and the only one that needs no help. Two
# installations put it elsewhere:
#
#   * rustup, whose shims live in ~/.cargo/bin and are reached through a PATH
#     entry that rustup's own ~/.cargo/env sets. If that file is missing the
#     shims are unreachable even when the toolchains are fine.
#   * Homebrew's `rustup` formula, which is KEG-ONLY -- it conflicts with the
#     `rust` formula -- so nothing of it is linked into /opt/homebrew/bin except
#     rustup itself.
#
# Neither is required. Homebrew's `rust` formula installs cargo and rustc
# directly onto PATH and needs none of this.
if ! command -v cargo >/dev/null 2>&1; then
	for _d in "$HOME/.cargo/bin" /opt/homebrew/opt/rustup/bin; do
		if [ -x "$_d/cargo" ]; then
			PATH="$_d:$PATH"
			export PATH
			echo "note: no cargo on PATH; using $_d/cargo"
			break
		fi
	done
fi
if ! command -v cargo >/dev/null 2>&1; then
	echo "check: no cargo found." >&2
	echo "  Install Rust however you prefer. Without rustup:" >&2
	echo "    brew install rust" >&2
	echo "  With rustup, ensure its bin directory is on PATH." >&2
	exit 2
fi

# --- is the toolchain built for this machine? --------------------------------
# rust-toolchain.toml pins the CHANNEL, not the architecture, and that is
# correct: the architecture should follow the host so the same file works on
# Linux CI. But it means a machine whose rustup default-host is wrong builds an
# emulated binary from a correctly pinned channel -- and build.rs stamps
# x-engine-build.xon from the TARGET, so the contract parameters x-lang reads
# follow the mistake silently.
#
# Report it rather than fail: an emulated build is wrong for this machine, not
# invalid.
_host="$(uname -m)"
case "$_host" in arm64|aarch64) _want=aarch64 ;; *) _want="$_host" ;; esac
_have="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
case "$_have" in
	*"$_want"*) ;;
	"") ;;
	*)
		echo "warning: building for $_have on a $_host host -- an emulated binary."
		echo "         build.rs stamps x-engine-build.xon from the target, so the"
		echo "         reported machine follows this."
		echo "         Fix: rustup set default-host $_want-apple-darwin"
		;;
esac

# --all / --workspace throughout: the engine is TWO crates, and the second is the
# one that carries every line of unsafe code. A gate that checked only the root
# package would leave exactly the crate most worth checking unchecked.
echo "== rustfmt =="
cargo fmt --all --check

echo "== clippy =="
# Warnings are errors here. A warning nobody has to fix is a warning that
# accumulates, and this repo has already demonstrated that.
cargo clippy --workspace --all-targets -- -D warnings

echo "== unit tests =="
cargo test --workspace --quiet

# Doctests are why this crate has a lib target. They are skipped entirely for a
# binary, so every example in the documentation went unchecked until there was
# one -- and a doc comment making a claim about behaviour is an unverified
# assertion until something runs it.
echo "== doctests =="
cargo test --workspace --doc --quiet

echo "== release build =="
cargo build --release --quiet

# The conformance suite is x-lang's, not this repo's, so it runs only when a
# checkout is pointed at. It answers a DIFFERENT question from the tests above:
# those ask whether a primitive does what this engine intends, the suite asks
# whether the intent is x-lang's. Both are needed and neither substitutes.
if [ -n "$XLANG" ]; then
	echo "== conformance (x-lang: $XLANG) =="
	HERE="$(pwd)"
	( cd "$XLANG" \
	  && X_BIN="$HERE/target/release/x-engine" X_ENGINE_DIR="$HERE" \
	     sh tests/x/conformance/runner.sh 2>&1 | tail -1 )
fi

echo "OK"
