//! The process I/O boundary.
//!
//! Writing needs only the object model — stdout is a process resource, not part
//! of the engine — so `write-str` sits at the objects level. READING is different:
//! it consumes the same stream the program arrived on, so it needs the engine.
//! The two signatures say so.
//!
//! The reading instructions work on the SAME stream the program arrived on. The engine
//! owns its reader for that reason: what `read-char` should answer is whatever is
//! left after the form being evaluated, which a reader living in `main` could not
//! be asked, and an engine reading real stdin would find already consumed.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::EnvId;
use crate::obj::{Obj, NIL};
use crate::objects::Objects;
use crate::prim::PrimDef;
use std::io::Write;

/// The OUT port: raw bytes of a string to the current output, and NIL rather
/// than a count. Nothing in x-lang's library reads the return, which is exactly
/// why it is easy to get wrong in either direction and why the conformance case
/// asserts the side effect and the answer in one breath.
fn write_str(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let text = a_.str_val(a[0]);
    let out = std::io::stdout();
    let mut h = out.lock();
    let _ = h.write_all(text.as_bytes());
    let _ = h.flush();
    Ok(NIL)
}

/// One byte from the input stream; nil at end of input. A NUL byte read from the
/// stream is a char like any other, so exhaustion and a zero byte stay distinct.
fn read_char(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    match e.read_byte() {
        Some(b) => Ok(e.objects.char_new(b as u32)),
        None => Ok(NIL),
    }
}

/// `(io read)` — one FORM, unevaluated. NIL at end of input.
///
/// Folding end-of-input to nil loses the difference between reading the value
/// `()` and running out, and that is deliberate: a caller of `read` has no use
/// for the distinction, and the reference folds it the same way.
fn read_form(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    Ok(e.read_form()?.unwrap_or(NIL))
}

/// `(io repl-read)` — the same act, but end of input arrives as `%token-eof`.
///
/// THE ONE DIFFERENCE between this and `read`, and it is not prompting or echo —
/// those really do belong to the library. It is that a REPL needs THREE
/// outcomes where a reader needs two: a value (nil included, since `()` reads as
/// nil and simply evaluates), a clean end of input, and a truncated form, which
/// arrives as a raise. Folding the sentinel here would merge the first two, and
/// a REPL that cannot tell `()` from ctrl-d either exits on a valid form or
/// never exits at all.
fn repl_read(e: &mut Engine, _base: Obj, _a: &[Obj]) -> EvalResult {
    Ok(e.read_form()?.unwrap_or(e.token_eof))
}

/// `(include "path")` — read a file and evaluate its forms AT TOP LEVEL.
///
/// The caller's environment is deliberately NOT used, and this is the contract
/// rather than a shortcut. Every form in a loaded file is a top-level form, so:
///
/// * its `def`s must bind GLOBALLY. x-lang's own loader wraps `include` in a
///   `fn` (lib/x/boot/module.x makes the bare `include` relative-aware), so the
///   caller's env is that wrapper's activation frame — and a file's definitions
///   would land there and vanish when it returned. The symptom is not an error:
///   the include succeeds and the names are simply Unbound afterwards.
/// * a CLOSURE the file defines must not capture the includer's frames. The
///   reference engine records what that costs — a closure captures the env head,
///   so the loader wrapper's own formals (`path`, `name`) would shadow the
///   global env inside every loaded function forever.
///
/// The path resolves against the working directory, which is the convention
/// x-lang's harnesses rely on: they chdir to the engine root so a prelude can
/// include the engine's own committed base paths.
fn include(e: &mut Engine, args: Obj, env: EnvId) -> EvalResult {
    let form = e.nth(args, 0);
    // The PATH is evaluated in the caller's env — it is an ordinary argument,
    // and a caller computing one from a local is entitled to.
    let p = e.eval(form, env)?;
    let path = e.objects.str_val(p);
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Err(Cond::CannotInclude(path)),
    };
    let top = e.root_env();
    // HIDE what is pending, so every form in the file sees tail position exactly
    // as it would at the true top level. The reference does the same thing to
    // its save stack and says why: each form read from a file IS a top-level
    // form, and its `def`s must bind globally rather than as locals of whatever
    // was being evaluated when the load was triggered.
    //
    // Without this an included file's definitions land in the includer's frame
    // and vanish with it — and since x-lang's own loader wraps `include` in a
    // `fn`, that is every file the library loads.
    let outer = e.hide_pending();
    let r = e.eval_source(&src, top);
    e.restore_pending(outer);
    r
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::op("include", include),
    PrimDef::filed("io", "write-str", 1, write_str),
    PrimDef::filed_full("io", "read-char", 0, read_char),
    PrimDef::filed_full("io", "read", 0, read_form),
    PrimDef::filed_full("io", "repl-read", 0, repl_read),
];

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use crate::testkit::{raises, truthy, CATALOG};

    /// NIL, not a byte count. Both halves of the contract in one assertion, the
    /// way x-lang states it.
    #[test]
    fn write_str_answers_nil() {
        assert!(truthy(r#"(eq? ((%coord (lit io) (lit write-str)) "") ())"#));
    }

    /// A non-string operand is READ, not refused: its data word is taken as an
    /// address and the bytes there are written. Nonsense output, no raise —
    /// which is what a machine does with a bad address that happens to be mapped.
    #[test]
    fn a_non_string_operand_is_read_not_refused() {
        assert!(!raises("((%coord (lit io) (lit write-str)) 5)"));
    }

    /// Reading past the end answers nil rather than blocking or panicking. The
    /// test stream is whatever `eval_str` was handed, which is already exhausted
    /// by the time the last form runs.
    #[test]
    fn read_char_at_end_of_input_is_nil() {
        assert!(truthy("(eq? ((%coord (lit io) (lit read-char))) ())"));
    }

    /// THREE outcomes, not two. The whole reason the sentinel exists.
    ///
    /// Driven through the engine API rather than the source string, because the
    /// stream `io read` consumes is the ENGINE'S — the program text — and not
    /// the source a test happens to be evaluating.
    #[test]
    fn repl_read_tells_the_value_nil_apart_from_the_end_of_input() {
        let mut e = Engine::new();
        e.set_input("()");
        let read = e
            .eval_str(&format!("{}\n(%coord (lit io) (lit repl-read))", CATALOG))
            .expect("repl-read");

        // `()` reads as nil and is a VALUE. It must not look like an ending.
        let env = e.root_env();
        let first = e.call_with_values(read, &[], env).expect("first read");
        assert!(first.is_nil(), "() reads as nil");
        assert!(
            !e.objects.is_token_eof(first),
            "the value nil must not be the sentinel"
        );

        // Now the input really is exhausted.
        let second = e.call_with_values(read, &[], env).expect("second read");
        assert!(
            e.objects.is_token_eof(second),
            "end of input must answer %token-eof"
        );
    }

    /// And `read` folds it, which is why both instructions exist. Same exhausted
    /// stream, different answer.
    #[test]
    fn read_folds_the_end_of_input_where_repl_read_does_not() {
        let mut e = Engine::new();
        e.set_input("");
        let read = e
            .eval_str(&format!("{}\n(%coord (lit io) (lit read))", CATALOG))
            .expect("read");
        let env = e.root_env();
        let v = e.call_with_values(read, &[], env).expect("read at eof");
        assert!(v.is_nil(), "read folds end of input to nil");
        assert!(
            !e.objects.is_token_eof(v),
            "and does NOT answer the sentinel"
        );
    }

    /// Compared by IDENTITY, so it must not be `eq?`-confusable with a number
    /// or with nil — the conflation lib/x/repl/loop.x warns about.
    #[test]
    fn the_sentinel_is_not_confusable_with_nil_or_a_number() {
        assert!(truthy("(match ((eq? %token-eof ()) ()) (#t 1))"));
        assert!(truthy("(match ((eq? %token-eof 0) ()) (#t 1))"));
        assert!(truthy("(same? %token-eof %token-eof)"));
    }
}
