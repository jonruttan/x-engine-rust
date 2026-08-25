//! Objects: identity, pairs, and the type registry.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
use crate::obj::Obj;
use crate::objects::Objects;
use crate::prim::PrimDef;
/// `eq?` compares the OPERAND WORD, not the type: `a == b || (both non-nil &&
/// word(a) == word(b))`, as `x_prim_eq` does. A CHARACTER therefore equals the
/// INTEGER of its code, which the string printer's escape tables rely on.
/// Strings compare by identity for free — a string's word is the address of
/// its bytes. Objects that merely share a first word conflate here; identity
/// questions belong to `same?`.
fn eq(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    // THE OPERAND WORD, NOT THE TYPE. The reference is one expression:
    //
    //     a == b || (!isnil(a) && !isnil(b) && x_intval(a) == x_intval(b))
    //
    // It reads slot 0 of BOTH operands and compares the words. It does not ask
    // whether the two are the same kind — so a CHARACTER and the INTEGER of its
    // code are `eq?`, which x-lang's printer depends on: `%print-str-esc?` and
    // `%print-str-esc-byte` (lib/x/boot/printer.x) are handed
    // `(str byte-ref s i)` — a character — and match it against 34, 92, 10, 9,
    // 13. Type-gating the comparison made every one of those arms miss, so a
    // quote printed unescaped, a newline came out `\x0a`, and a carriage return
    // lost its backslash. That is the whole of the csv/json `parse` cluster.
    //
    // Strings stay identity-compared for free: slot 0 of a string is the address
    // of its bytes, so two equal strings hold different words.
    //
    // It DOES conflate objects that merely share a first word — two distinct
    // closures answer #t here. That is the reference's behaviour and x-lang knows
    // it: tower-compiled.x warns that an `eq?`-keyed analyser swap "stamped the
    // first compiled handler over every seat", and uses `obj same?` instead.
    let same = a[0] == a[1]
        || (!a[0].is_nil() && !a[1].is_nil() && a_.data(a[0], 0).raw() == a_.data(a[1], 0).raw());
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

/// `(type make name parent)` — a new type, FILED where the library can find it.
///
/// Registration is not optional and not a courtesy. `lib/x/type/struct.x`'s
/// `type by-atom` is the only way the library reaches a type's tree, and it
/// walks the base's type-alist; a type that never lands there answers nil, and
/// callers write into what they get back rather than checking. That is how
/// `lib/x/type/promise.x` came to push a call handler through nil.
fn type_make(e: &mut Engine, base: Obj, a: &[Obj]) -> EvalResult {
    // The NAME arrives as a string; the handle is made from it here, and the
    // handle is what comes back. x-lang keeps the type-alist keyed by it and
    // passes it to `make-instance`, so answering the tree would hand the library
    // something its own accessors do not expect.
    let text = e.objects.str_val(a[0]);
    let name = e.objects.handle(&text);
    let t = e.objects.type_new(name, a[1]);
    e.file_type_in(base, t);
    Ok(name)
}

/// `(type of v)` — the type handle, FILING it if this is the first sight of it.
///
/// Builtin types are made on demand, and one that is never filed cannot be
/// reached: `type by-atom` walks the base's alist, answers nil, and callers
/// write into the nil. That is how `lib/x/type/iter.x` came to push a write
/// handler through nothing.
///
/// Filing here rather than at each construction site is deliberate: this
/// instruction is the only door x-lang has to a type, so anything the library
/// can name has passed through it.
fn type_of(e: &mut Engine, base: Obj, a: &[Obj]) -> EvalResult {
    let t = e.objects.type_of(a[0]);
    for fresh in e.objects.take_unfiled_types() {
        e.file_type_in(base, fresh);
    }
    Ok(t)
}

fn type_is(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let got = a_.type_of(a[0]);
    let t = a[1];
    Ok(a_.truth(!t.is_nil() && got == t))
}

/// `(type make-instance handle data)` — an instance of a registered type.
///
/// The handle is resolved to its TREE through the base, because the type word
/// must hold the tree: the library dereferences it and checks the tree tag
/// before walking.
/// TWO slots, not one, and the second one matters.
///
/// The reference allocates a PAIR-SIZED instance — `X_OBJ_LENGTH_PAIR`, data in
/// the first slot and NULL in the second — and x-lang uses that second slot:
/// `%class-hot` in lib/x/type/class.x caches a class's flattened member table
/// there with `(rest class)` and `%set-rest!`.
///
/// With one slot, `rest` read PAST THE END of the object — into the next
/// allocation's header. That was invisible while type words were nil: the
/// garbage read as nil, so `%class-hot` saw an empty cache and rebuilt it
/// correctly every time. Stamping the type word made the same read return a type
/// TREE, which `%class-hot` then returned AS the cached table, and every class
/// answered "no such static member".
///
/// The stamping did not cause that. It exposed it.
fn make_instance(e: &mut Engine, base: Obj, a: &[Obj]) -> EvalResult {
    let tree = e.resolve_tree_in(base, a[0]);
    let o = e.objects.instance(tree, 2);
    e.objects.set_data(o, 0, a[1].word());
    Ok(o)
}

/// The type word is written from the operand, whatever it is. Whether that
/// operand is a REGISTERED type is x-lang's question to ask.
fn obj_make(e: &mut Engine, _base: Obj, a: &[Obj]) -> EvalResult {
    let tree = e.resolve_tree(a[0]);
    let n = e.objects.as_int(a[1]).max(0) as usize;
    Ok(e.objects.instance(tree, n))
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

crate::uniform_value!(eq_u, eq, 2);
crate::uniform_value!(same_u, same, 2);
crate::uniform_value!(first_u, first, 1);
crate::uniform_value!(rest_u, rest, 1);
crate::uniform_value!(pair_u, pair, 2);
crate::uniform_engine!(type_make_u, type_make, 2);
crate::uniform_engine!(type_of_u, type_of, 1);
crate::uniform_value!(type_is_u, type_is, 2);
crate::uniform_engine!(make_instance_u, make_instance, 2);
crate::uniform_engine!(obj_make_u, obj_make, 2);
crate::uniform_value!(make_callable_u, make_callable, 1);

#[rustfmt::skip]
pub const TABLE: &[PrimDef] = &[
    PrimDef::row(Some("eq?"), Some(("obj", "eq?")), 2, eq_u),
    PrimDef::row(Some("same?"), Some(("obj", "same?")), 2, same_u),
    PrimDef::row(Some("first"), None, 1, first_u),
    PrimDef::row(Some("rest"), None, 1, rest_u),
    PrimDef::row(Some("pair"), None, 2, pair_u),
    PrimDef::row(Some("make-type"), Some(("type", "make")), 2, type_make_u),
    PrimDef::row(Some("type-of"), Some(("type", "of")), 1, type_of_u),
    PrimDef::row(Some("type?"), Some(("type", "?")), 2, type_is_u),
    PrimDef::row(Some("make-instance"), Some(("type", "make-instance")), 2, make_instance_u),
    PrimDef::row(None, Some(("obj", "make")), 2, obj_make_u),
    PrimDef::row(None, Some(("obj", "make-callable")), 1, make_callable_u),
];

#[cfg(test)]
mod tests {
    use crate::testkit::{eval_ok, truthy};

    /// The line x-engine-c draws, asserted in both directions so a future
    /// "simplification" to pure pointer identity fails here rather than as a
    /// number in a conformance count.
    /// A CHARACTER equals the INTEGER of its code, because `eq?` compares the
    /// operand word and not the type. x-lang's string printer is built on it.
    #[test]
    fn eq_crosses_char_and_int() {
        assert!(truthy(r"(eq? #\A 65)"));
        assert!(!truthy(r"(eq? #\A 66)"));
        // same? is identity and must NOT cross.
        assert!(!truthy(r"(same? #\A 65)"));
    }

    #[test]
    fn eq_compares_numbers_by_value_and_strings_by_identity() {
        assert!(truthy("(eq? 1 1)"));
        assert!(!truthy("(eq? 1 2)"));
        assert!(!truthy(r#"(eq? "a" "a")"#));
        assert!(truthy("(eq? () ())"));
        assert!(truthy("(eq? (lit a) (lit a))"), "symbols are interned");
    }

    /// TRUTHINESS IS NOT ENOUGH: a symbol and nil branch exactly like `#t`
    /// and `#f`, and a predicate's answer is a VALUE that gets displayed. So
    /// assert IDENTITY with the `#t`/`#f` objects, not truthiness.
    #[test]
    fn a_predicate_answers_the_very_objects_hash_t_and_hash_f() {
        assert!(truthy("(same? (eq? 1 1) #t)"));
        assert!(truthy("(same? (eq? 1 2) #f)"));
        assert!(
            truthy("(same? (same? 1 1) #f)"),
            "same? answers with them too"
        );
        assert!(truthy("(same? (< 1 2) #t)"));
        assert!(truthy("(same? (< 2 1) #f)"));
    }

    /// FALSE IS `#f`, NOT NIL. The reference returns its base's false field from
    /// every predicate; answering nil is a different value that merely behaves
    /// the same in a conditional.
    #[test]
    fn a_false_answer_is_not_nil() {
        assert!(!truthy("(eq? (eq? 1 2) ())"));
        assert!(truthy("(same? (eq? 1 2) #f)"));
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
