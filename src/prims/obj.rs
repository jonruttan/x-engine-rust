//! Objects: identity, pairs, and the type registry.

use crate::diag::Cond;
use crate::obj::Obj;
use crate::objects::Objects;
use crate::prim::PrimDef;

/// Identity, EXCEPT for numbers. Symbols are interned and nil is one value, so
/// identity is right for those — but an integer is a boxed object here, so two
/// spellings of 1 are two objects and a pure pointer compare answers false.
///
/// x-engine-c was asked rather than guessed at, and it draws the line in a
/// specific place: `(eq? 1 1)` holds and `(eq? "a" "a")` does NOT. Numbers
/// compare by value; strings, which are mutable, by identity.
fn eq(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let same = if a_.is_int(a[0]) && a_.is_int(a[1]) {
        a_.int_val(a[0]) == a_.int_val(a[1])
    } else {
        a[0] == a[1]
    };
    Ok(a_.truth(same))
}

/// STRICT identity, and the reason it exists beside `eq?`: eq? answers by value
/// for numbers, so it cannot distinguish two objects that merely hold the same
/// one. Where identity is the actual question — telling two handlers apart —
/// x-lang reaches for same?.
fn same(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.truth(a[0] == a[1]))
}

/// UNCHECKED by ruling: undefined on a non-pair, guarded at the call site.
///
/// That is not pedantry about a rule. x-lang reads a custom instance's payload
/// with a plain `first`, so an implementation that checked for a pair and
/// answered nil otherwise would REFUSE THE LANGUAGE'S OWN OBJECT PROTOCOL while
/// looking like a safety improvement. It did, once, and the symptom was an
/// instance whose payload read as nil.
fn first(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.first(a[0]))
}

fn rest(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.rest(a[0]))
}

fn pair(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.pair(a[0], a[1]))
}

fn type_make(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.type_new(a[0], a[1]))
}

fn type_of(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    Ok(a_.type_of(a[0]))
}

fn type_is(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let got = a_.type_of(a[0]);
    let t = a[1];
    Ok(a_.truth(!t.is_nil() && got == t))
}

fn make_instance(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let o = a_.instance(a[0], 1);
    a_.set_data(o, 0, a[1].word());
    Ok(o)
}

/// The type word is written from the operand, whatever it is. Whether that
/// operand is a REGISTERED type is x-lang's question to ask.
fn obj_make(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let n = a_.as_int(a[1]).max(0) as usize;
    Ok(a_.instance(a[0], n))
}

/// `(obj make-callable p)` — a raw address dressed as callable.
///
/// Answers a value that can sit at the head of a form. Calling it does what
/// calling any non-combiner does — the form is data — because the `core`
/// profile has no foreign door and there is nothing to jump to. An engine with
/// one makes this callable without changing what it constructs.
fn make_callable(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let at = a_.as_ptr(a[0]);
    Ok(a_.foreign(at.raw()))
}

pub const TABLE: &[PrimDef] = &[
    PrimDef::both("eq?", "obj", "eq?", 2, eq),
    PrimDef::both("same?", "obj", "same?", 2, same),
    PrimDef::bare("first", 1, first),
    PrimDef::bare("rest", 1, rest),
    PrimDef::bare("pair", 2, pair),
    PrimDef::filed("type", "make", 2, type_make),
    PrimDef::filed("type", "of", 1, type_of),
    PrimDef::filed("type", "?", 2, type_is),
    PrimDef::filed("type", "make-instance", 2, make_instance),
    PrimDef::filed("obj", "make", 2, obj_make),
    PrimDef::filed("obj", "make-callable", 1, make_callable),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{eval_ok, truthy};

    /// The line x-engine-c draws, asserted in both directions so a future
    /// "simplification" to pure pointer identity fails here rather than as a
    /// number in a conformance count.
    #[test]
    fn eq_compares_numbers_by_value_and_strings_by_identity() {
        assert!(truthy("(eq? 1 1)"));
        assert!(!truthy("(eq? 1 2)"));
        assert!(!truthy(r#"(eq? "a" "a")"#));
        assert!(truthy("(eq? () ())"));
        assert!(truthy("(eq? (lit a) (lit a))"), "symbols are interned");
    }

    /// same? is strictly identity, which is the whole of its difference from eq?.
    #[test]
    fn same_does_not_collapse_equal_numbers() {
        assert!(!truthy("(same? 1 1)"));
        assert!(truthy("(def x 1) (same? x x)"));
    }

    /// The regression that motivated making these unchecked: a custom instance's
    /// payload is read with a plain `first`, so a `first` that guarded on pair
    /// answered nil and broke the object protocol while looking safer.
    #[test]
    fn first_reads_an_instances_payload_not_only_pairs() {
        let (e, v) = eval_ok(
            r#"
            (def T ((%coord (lit type) (lit make)) "PAYLOAD" ()))
            (def v ((%coord (lit type) (lit make-instance)) T (pair 7 8)))
            (first (first v))
        "#,
        );
        assert!(
            e.objects.is_int(v),
            "an instance's payload must be readable"
        );
        assert_eq!(e.objects.int_val(v), 7);
    }

    #[test]
    fn first_and_rest_of_nil_are_nil() {
        assert!(truthy("(eq? (first ()) ())"));
        assert!(truthy("(eq? (rest ()) ())"));
    }

    /// Simple values carry no type word, so stability comes from a table keyed by
    /// flags. Without it every ask would mint a fresh type object and `type ?`
    /// would answer false for a value against its own type.
    #[test]
    fn type_of_is_stable_per_kind_and_distinguishes_kinds() {
        assert!(truthy(
            "(def %tof (%coord (lit type) (lit of))) (same? (%tof 1) (%tof 2))"
        ));
        assert!(!truthy(
            r#"(def %tof (%coord (lit type) (lit of))) (same? (%tof 1) (%tof "s"))"#
        ));
        assert!(!truthy(
            "(def %tof (%coord (lit type) (lit of))) (same? (%tof 1) (%tof (pair 1 2)))"
        ));
    }

    #[test]
    fn type_predicate_agrees_with_type_of() {
        assert!(truthy(
            "(def %tof (%coord (lit type) (lit of)))
             (def %is (%coord (lit type) (lit ?)))
             (%is 1 (%tof 1))"
        ));
        assert!(!truthy(
            r#"(def %tof (%coord (lit type) (lit of)))
               (def %is (%coord (lit type) (lit ?)))
               (%is 1 (%tof "s"))"#
        ));
    }

    /// The type word is written from the operand, whatever it is. Whether that
    /// operand is a REGISTERED type is x-lang's question — an engine that
    /// refused here would be enforcing a type system it does not have.
    #[test]
    fn obj_make_writes_whatever_type_word_it_is_given() {
        assert!(truthy(
            "(def x ((%coord (lit obj) (lit make)) 5 2))
             (match ((eq? x ()) ()) (#t 1))"
        ));
    }

    #[test]
    fn an_instance_reports_the_type_it_was_made_with() {
        assert!(truthy(
            r#"
            (def %tof (%coord (lit type) (lit of)))
            (def T ((%coord (lit type) (lit make)) "CONFORM" ()))
            (same? (%tof ((%coord (lit obj) (lit make)) T 2)) T)
        "#
        ));
    }
}
