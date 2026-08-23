//! Conditions, and how they are shown.
//!
//! Two things were fused before this file and are separated here.
//!
//! **What went wrong** is a SHORT LIST, and that is the point. An engine is a
//! machine: it reads words and applies operators. It does not know about types,
//! it does not count arguments, and it has no opinion about dividing by zero —
//! those belong to x-lang, one layer up.
//!
//! This file used to carry eight `Kind` variants, an arity checker and a
//! divide-by-zero policy, each individually defensible and collectively a layer
//! violation: checking pulled DOWN into the machine because the machine could do
//! it. x-lang's contract had already ruled `first`/`rest` unchecked; everything
//! else here was the same rule, unapplied.
//!
//! What remains is what a machine can actually fail at: a name with no binding
//! has no word to read, a file may not open, and stdin may not be readable.
//!
//! **How it is shown** was `eprintln!("*** ERROR: ...")` written twice in main.
//! That prefix is contract layer E — the engine owes it to x-lang's wrapper — and
//! it was a bare literal with no name, assumed to be correct for all time. It has
//! one home now, [`PREFIX`], and nothing outside this file writes it.
//!
//! The split matters because the two have different dependencies. A condition is
//! DATA and can be constructed anywhere. Rendering one needs the objects, because a
//! raised value is an object and its text lives in storage. Fusing them is what
//! forced every failure site to allocate a message immediately, whether anyone
//! would ever read it or not.

use crate::obj::Obj;
use crate::objects::Objects;
use std::fmt;

/// Contract layer E: diagnostics go to STDERR prefixed with this, and x-lang's
/// wrapper reads it. THE only place it is written.
pub const PREFIX: &str = "*** ERROR: ";

/// A condition in flight.
///
/// `Debug` is derived, which is not decoration: without it `Result<Obj, _>` has
/// no `expect`, and every test in this repo had to write `.ok().expect(...)` to
/// get a failure message.
#[derive(Debug)]
pub enum Cond {
    /// `(error x)` — the program raised this value deliberately. The VALUE is
    /// carried, not its text, because `guard` binds it and a handler that only
    /// ever saw a string could not tell an error from the spelling of one.
    Raised(Obj),
    /// A name with no binding. Carries the symbol.
    Unbound(Obj),
    /// `include` could not read a file.
    CannotInclude(String),
    /// `tail-eval` was handed something that is not an environment.
    NotAnEnvironment(Obj),
    /// The program could not be read from stdin at all.
    NoProgram,
    /// The armed allocation ceiling was passed.
    AllocLimit,
}

impl Cond {
    /// The text of the diagnostic, without the prefix.
    ///
    /// Takes the objects because a condition's text may live in storage — that is
    /// precisely why this is not a `Display` impl on `Cond` itself.
    pub fn message(&self, a: &Objects) -> String {
        match self {
            Cond::Raised(v) => value_text(a, *v),
            Cond::Unbound(sym) => format!("Unbound SYMBOL '{}", a.sym_name(*sym)),
            Cond::CannotInclude(path) => format!("cannot include {}", path),
            Cond::NotAnEnvironment(_) => "tail-eval: not an environment".to_string(),
            Cond::NoProgram => "cannot read program from stdin".to_string(),
            Cond::AllocLimit => "allocation limit exceeded".to_string(),
        }
    }

    /// The value `guard` binds to its handler's name.
    ///
    /// A deliberate raise hands back exactly what the program raised. Everything
    /// else becomes a string, and only at this moment — a condition that is
    /// caught and ignored never allocates one, which the old code could not
    /// avoid because it built the message at the failure site.
    pub fn value(&self, a: &mut Objects) -> Obj {
        match self {
            Cond::Raised(v) => *v,
            other => {
                let text = other.message(a);
                a.str_new(&text)
            }
        }
    }

    /// The full stderr line, prefix included. Borrows the objects for as long as
    /// the renderer lives, in the manner of `Path::display`.
    pub fn diagnostic<'a>(&'a self, objects: &'a Objects) -> Diagnostic<'a> {
        Diagnostic {
            cond: self,
            objects,
        }
    }
}

/// A condition paired with the objects needed to render it.
///
/// `Cond` deliberately does NOT implement `Display`, and this is why: a
/// condition genuinely cannot render itself, because its values live in storage
/// it does not own. Implementing `Display` on `Cond` would mean degrading the
/// raised-value case to something like "<value>" — a worse diagnostic in exchange
/// for satisfying a trait. This carries the missing half instead.
pub struct Diagnostic<'a> {
    cond: &'a Cond,
    objects: &'a Objects,
}

impl fmt::Display for Diagnostic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", PREFIX, self.cond.message(self.objects))
    }
}

impl fmt::Debug for Diagnostic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Diagnostic<'_> {}

/// Render a value as diagnostic text.
///
/// Public because a raised value's text is wanted in places other than a
/// diagnostic line — the same rendering, without the prefix.
pub fn value_text(a: &Objects, v: Obj) -> String {
    if v.is_nil() {
        String::new()
    } else if a.is_str(v) || a.is_sym(v) {
        a.str_val(v)
    } else if a.is_int(v) {
        format!("{}", a.int_val(v))
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prefix is contract layer E. x-lang's conformance runner matches on it,
    /// so this is not a formatting preference -- it is the agreement.
    #[test]
    fn the_diagnostic_line_carries_the_contract_prefix() {
        let mut a = Objects::new();
        let sym = a.sym("nope");
        let line = Cond::Unbound(sym).diagnostic(&a).to_string();
        assert!(line.starts_with(PREFIX));
        assert_eq!(line, "*** ERROR: Unbound SYMBOL 'nope");
    }

    /// Matched against x-engine-c deliberately, and asserted here so a reworded
    /// message cannot drift away from the reference engine unnoticed.
    #[test]
    fn the_unbound_message_matches_the_reference_engine() {
        let mut a = Objects::new();
        let sym = a.sym("nope");
        assert_eq!(Cond::Unbound(sym).message(&a), "Unbound SYMBOL 'nope");
    }

    /// A deliberate raise hands back the VALUE, not its text. `(guard (e e)
    /// (error (lit boom)))` must bind the symbol, not a string spelled "boom".
    #[test]
    fn a_raised_value_survives_being_caught() {
        let mut a = Objects::new();
        let sym = a.sym("boom");
        assert_eq!(Cond::Raised(sym).value(&mut a), sym);
    }

    /// Everything else becomes a string, and only when someone asks.
    #[test]
    fn a_machine_condition_becomes_text_when_caught() {
        let mut a = Objects::new();
        let v = Cond::CannotInclude("nope.x".into()).value(&mut a);
        assert!(a.is_str(v));
        assert_eq!(a.str_val(v), "cannot include nope.x");
    }
}
