//! THE LANGUAGE'S NAMES, in one place.
//!
//! Every string x-lang can see — type names, handler families, character names,
//! the `ffi call` conventions — is declared here and nowhere else.
//!
//! # Why one place
//!
//! These were scattered across twenty files as literals. Nothing was wrong with
//! any single one of them; what was wrong was that changing the initial syntax,
//! or emitting diagnostics in another language, meant reading every module to
//! find which literals were vocabulary and which were Rust. The names are DATA
//! about x-lang, not facts about this implementation, and data belongs somewhere
//! you can enumerate.
//!
//! The instruction names are the exception, and deliberately: they stay in the
//! `PrimDef` tables beside the functions they name, because a coordinate split
//! from its implementation is how the two drift. `tools/check/isa.sh` is what
//! keeps THOSE honest — it asks the built engine whether every row of isa.x
//! resolves, rather than trusting either side.

/// A handler family: the slot a type keeps one handler stack in.
///
/// An enum rather than a string. These are looked up on the reader's hot path
/// and on every value-call dispatch, and a string comparison there is both
/// slower and unable to fail at compile time — a typo'd `"analsye"` returned nil
/// and read as "no handler installed".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Mark,
    Make,
    Free,
    Clone,
    Units,
    Length,
    Call,
    Eval,
    From,
    To,
    Analyse,
    Delimit,
    Read,
    Write,
    Display,
    Iter,
    Ops,
    Data,
}

impl Family {
    /// The name x-lang knows this family by, as `type make`'s handler alist
    /// keys it.
    pub fn name(self) -> &'static str {
        match self {
            Family::Mark => "mark",
            Family::Make => "make",
            Family::Free => "free",
            Family::Clone => "clone",
            Family::Units => "units",
            Family::Length => "length",
            Family::Call => "call",
            Family::Eval => "eval",
            Family::From => "from",
            Family::To => "to",
            Family::Analyse => "analyse",
            Family::Delimit => "delimit",
            Family::Read => "read",
            Family::Write => "write",
            Family::Display => "display",
            Family::Iter => "iter",
            Family::Ops => "ops",
            Family::Data => "data",
        }
    }

    /// The family a name denotes, or `None` for a name x-lang does not use.
    ///
    /// An unknown key is not an error here: which keys exist is x-lang's
    /// vocabulary, and an engine that refused one would be ruling on a question
    /// one layer up.
    pub fn from_name(name: &str) -> Option<Family> {
        ALL.iter().copied().find(|f| f.name() == name)
    }
}

/// Every family, in no particular order — the type's layout is base-paths.x's
/// business, not this list's.
pub const ALL: &[Family] = &[
    Family::Mark,
    Family::Make,
    Family::Free,
    Family::Clone,
    Family::Units,
    Family::Length,
    Family::Call,
    Family::Eval,
    Family::From,
    Family::To,
    Family::Analyse,
    Family::Delimit,
    Family::Read,
    Family::Write,
    Family::Display,
    Family::Iter,
    Family::Ops,
    Family::Data,
];

/// The nine named characters, which are the reference engine's list.
///
/// `lib/` writes `#\newline` and expects a character back, so these are the
/// language's names rather than this engine's choice.
pub const CHAR_NAMES: &[(&str, u32)] = &[
    ("alarm", 7),
    ("backspace", 8),
    ("delete", 127),
    ("escape", 27),
    ("newline", 10),
    ("null", 0),
    ("return", 13),
    ("space", 32),
    ("tab", 9),
];

/// How x-lang writes a value with no readable form: `#<…>`.
///
/// docs/syntax.md's printed-forms table gives this as the opaque shape --
/// "fn / op / dict / instance | `#<…>` opaque -- deliberately not pasteable".
/// It is a FORMAT and the thing inside it is a VALUE, which is why neither is
/// spelled out case by case: the engine reads the type's own name and wraps it.
pub const OPAQUE_OPEN: &str = "#<";
pub const OPAQUE_CLOSE: &str = ">";

/// What stands in when a value carries no type to name.
pub const UNNAMED: &str = "?";

/// Wrap a name in the opaque form.
pub fn opaque(name: &str) -> String {
    format!("{}{}{}", OPAQUE_OPEN, name, OPAQUE_CLOSE)
}

// --- what a diagnostic SAYS ---------------------------------------------------
//
// The engine reports conditions; the WORDS are here so they can be replaced
// without reading the code that raises them. x-expr's rule is stronger still --
// "the embedder supplies messages as data" -- and this is the shape that rule
// takes in an engine that must still say something when nothing has been
// supplied.
//
// `{}` is the value the condition carries.
pub const MSG_CANNOT_INCLUDE: &str = "cannot include {}";
pub const MSG_NOT_AN_ENV: &str = "tail-eval: not an environment";
pub const MSG_NO_PROGRAM: &str = "cannot read program from stdin";
pub const MSG_ALLOC_LIMIT: &str = "allocation limit exceeded";

/// How a CHARACTER is written: `#\` and the character itself.
pub const CHAR_PREFIX: &str = "#\\";
/// A character with no printable form.
pub const CHAR_UNKNOWN: &str = "?";

// --- names the engine BINDS ---------------------------------------------------

/// The `%isa-values` names: things bound to objects rather than to callables.
///
/// Declared in `tools/contract/isa.x` too, which is the manifest x-lang reads;
/// `tools/check/isa.sh` proves the two agree by asking the built engine.
/// The truth answer of every predicate, and the name that evaluates to it.
/// What an interrupt raises, so a guard can recognise it.
pub const MSG_STOP: &str = "STOP";

pub const TRUE: &str = "#t";
pub const FALSE: &str = "#f";
pub const ARGS: &str = "args";
pub const TOKEN_EOF: &str = "%token-eof";
pub const SIGINT_FLAG: &str = "%sigint-flag";
/// The evaluator-state routes on the base spine.
pub const ROUTE_SAVE_STACK: &str = "save-stack";
pub const ROUTE_TCO_EXPR: &str = "tco-expr";
pub const ROUTE_TCO_ENV: &str = "tco-env";
pub const ROUTE_SIGINT: &str = "sigint";
pub const ROUTE_ERROR_HANDLER: &str = "error-handler";
pub const X_MACHINE: &str = "x-machine";
pub const X_VERSION: &str = "x-version";
pub const X_RELEASE: &str = "x-release";

/// The quoting operative, which the reader's `'` macro expands to.
pub const LIT: &str = "lit";

// --- the printed forms of the non-finite numbers -------------------------------
// C's spellings, because `d->s` is `%.15g` and these are what it prints.
pub const NAN: &str = "nan";
pub const INF: &str = "inf";
pub const NEG_INF: &str = "-inf";

/// The `ffi call` ARITHMETIC and COMPARISON conventions, which call nothing.
pub const CONV_ADD: &str = "d+d";
pub const CONV_SUB: &str = "d-d";
pub const CONV_MUL: &str = "d*d";
pub const CONV_DIV: &str = "d/d";
pub const CONV_LT: &str = "d<d";
pub const CONV_GT: &str = "d>d";
pub const CONV_EQ: &str = "d=d";
pub const CONV_LE: &str = "d<=d";
pub const CONV_GE: &str = "d>=d";

/// The `ffi call` conventions that CROSS the foreign door.
pub const CONV_D_TO_D: &str = "d->d";
pub const CONV_DD_TO_D: &str = "dd->d";
pub const CONV_S0_TO_D: &str = "s0->d";
/// And the casts, which call nothing.
pub const CONV_I_TO_D: &str = "i->d";
pub const CONV_D_TO_I: &str = "d->i";
pub const CONV_D_TO_S: &str = "d->s";

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips, so a family cannot be spelled one way here and another at a
    /// call site.
    #[test]
    fn every_family_answers_to_its_own_name() {
        for f in ALL {
            assert_eq!(Family::from_name(f.name()), Some(*f), "for {:?}", f);
        }
    }

    /// Names are distinct: two families sharing one would make the second
    /// unreachable, silently.
    #[test]
    fn the_family_names_are_distinct() {
        let mut seen: Vec<&str> = ALL.iter().map(|f| f.name()).collect();
        seen.sort_unstable();
        let n = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), n, "two families share a name");
    }

    #[test]
    fn an_unknown_family_name_is_not_a_family() {
        assert_eq!(Family::from_name("analsye"), None);
        assert_eq!(Family::from_name(""), None);
    }
}
