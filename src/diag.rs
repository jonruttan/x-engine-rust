//! Conditions, and how they are shown.
//!
//! **What can go wrong** is a SHORT LIST, and that is the point. An engine is a
//! machine: it reads words and applies operators. It does not know about types,
//! it does not count arguments, and it has no opinion about dividing by zero —
//! those belong to x-lang, one layer up, and x-lang's contract rules
//! `first`/`rest` unchecked. What a machine can actually fail at: a name with
//! no binding has no word to read, a file may not open, stdin may not be
//! readable.
//!
//! **How it is shown** is contract layer E: diagnostics go to stderr with the
//! `*** ERROR: ` prefix the wrapper scrapes, named in `vocabulary.rs` like any
//! other spelling the outside world depends on.

use crate::obj::Obj;
use crate::objects::Objects;
use std::fmt;

/// Contract layer E: diagnostics go to STDERR prefixed with this, and x-lang's
/// wrapper reads it. THE only place it is written.
pub const PREFIX: &str = "*** ERROR: ";

/// A condition in flight.
///
/// `Debug` is derived so `Result<Obj, Cond>` has `expect`.
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
            Cond::Unbound(sym) => format!("Unbound SYMBOL '{}'", a.sym_name(*sym)),
            Cond::CannotInclude(path) => crate::vocabulary::MSG_CANNOT_INCLUDE.replace("{}", path),
            Cond::NotAnEnvironment(_) => crate::vocabulary::MSG_NOT_AN_ENV.to_string(),
            Cond::NoProgram => crate::vocabulary::MSG_NO_PROGRAM.to_string(),
            Cond::AllocLimit => crate::vocabulary::MSG_ALLOC_LIMIT.to_string(),
        }
    }

    /// The value `guard` binds to its handler's name.
    ///
    /// A deliberate raise hands back exactly what the program raised.
    /// Everything else becomes a string, and only at this moment — a condition
    /// that is caught and ignored never allocates a message at all.
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
    render(a, v, 0)
}

/// Render a raised value well enough to NAME the problem.
///
/// The printer proper is x-lang's — `lib/x/boot/printer.x` renders over the
/// `io write-str` door — and a bare engine has not loaded it. What this owes the
/// reader is not fidelity but IDENTIFICATION: an error is the last thing a run
/// says, and it is the only channel a bare engine has.
///
/// Pairs render recursively because that is how the library raises:
/// `(error (pair (lit unsupported-platform) x-machine))`. Three separate investigations here
/// started by having to find out what an empty error meant.
///
/// Depth-bounded because a raised structure may be cyclic, and an error handler
/// that hangs is worse than one that truncates.
fn render(a: &Objects, v: Obj, depth: usize) -> String {
    if v.is_nil() {
        return "()".to_string();
    }
    if a.is_str(v) || a.is_sym(v) {
        return a.str_val(v);
    }
    if a.is_int(v) {
        return format!("{}", a.int_val(v));
    }
    if a.is_char(v) {
        return match char::from_u32(a.as_char(v)) {
            Some(c) => format!("{}{}", crate::vocabulary::CHAR_PREFIX, c),
            None => format!(
                "{}{}",
                crate::vocabulary::CHAR_PREFIX,
                crate::vocabulary::CHAR_UNKNOWN
            ),
        };
    }
    if depth >= 4 {
        return "...".to_string();
    }
    if a.is_cell(v) {
        let mut parts = Vec::new();
        let mut at = v;
        while a.is_cell(at) && parts.len() < 8 {
            parts.push(render(a, a.first(at), depth + 1));
            at = a.rest(at);
        }
        if !at.is_nil() {
            parts.push(".".to_string());
            parts.push(render(a, at, depth + 1));
        } else if a.is_cell(v) && parts.len() == 8 {
            parts.push("...".to_string());
        }
        return format!("({})", parts.join(" "));
    }
    // Everything else: say WHAT it was, by READING its type rather than by
    // enumerating kinds here. Every value carries a pointer to its type and
    // every type carries its name, so the engine already knows the answer — a
    // list of cases in this file would be a second, staler copy of it, and a
    // kind added later would quietly print as the fallback.
    match a.type_name_of(v) {
        Some(name) => crate::vocabulary::opaque(&name),
        None => crate::vocabulary::opaque(crate::vocabulary::UNNAMED),
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
        assert_eq!(line, "*** ERROR: Unbound SYMBOL 'nope'");
    }

    /// Matched against x-engine-c deliberately, and asserted here so a reworded
    /// message cannot drift away from the reference engine unnoticed.
    #[test]
    fn the_unbound_message_matches_the_reference_engine() {
        let mut a = Objects::new();
        let sym = a.sym("nope");
        assert_eq!(Cond::Unbound(sym).message(&a), "Unbound SYMBOL 'nope'");
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
