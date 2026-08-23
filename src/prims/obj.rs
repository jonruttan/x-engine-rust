//! Objects: identity, pairs, and the type registry.

use crate::diag::Cond;
use crate::engine::Engine;
use crate::eval::EvalResult;
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
/// `(obj eq?)` — by VALUE for numbers and characters, by identity otherwise.
///
/// The character half was missing, and it is not a nicety: `%str-ref` answers a
/// freshly made character, so every string comparison in x-lang's library comes
/// down to `(eq? (%str-ref hay i) (%str-ref needle j))`. With identity those are
/// never equal, and `lib/x/platform/syscall.x` could not find "darwin" inside
/// "aarch64-apple-darwin" — the whole posix layer refused to load with
/// `(unsupported-platform . aarch64-apple-darwin)`.
///
/// Asked of x-engine-c rather than assumed:
///
/// ```text
/// (def %ic (prim-ref 'int '->char))
/// (match ((eq? (%ic 100) (%ic 100)) 'EQ) (#t 'NOT-EQ))   =>  'EQ
/// ```
///
/// Strings stay identity-compared, which the same interrogation confirmed
/// earlier: `(eq? "a" "a")` does not hold. The reference reads slot 0 either
/// way — for an atom that is its value, and for a string it is the pointer.
fn eq(a_: &mut Objects, a: &[Obj]) -> Result<Obj, Cond> {
    let same = if a_.is_int(a[0]) && a_.is_int(a[1]) {
        a_.int_val(a[0]) == a_.int_val(a[1])
    } else if a_.is_char(a[0]) && a_.is_char(a[1]) {
        a_.as_char(a[0]) == a_.as_char(a[1])
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

/// `(type make name parent)` — a new type, FILED where the library can find it.
///
/// Registration is not optional and not a courtesy. `lib/x/type/struct.x`'s
/// `type by-atom` is the only way the library reaches a type's tree, and it
/// walks the base's type-alist; a type that never lands there answers nil, and
/// callers write into what they get back rather than checking. That is how
/// `lib/x/type/promise.x` came to push a call handler through nil.
fn type_make(e: &mut Engine, a: &[Obj]) -> EvalResult {
    // The NAME arrives as a string; the handle is made from it here, and the
    // handle is what comes back. x-lang keeps the type-alist keyed by it and
    // passes it to `make-instance`, so answering the tree would hand the library
    // something its own accessors do not expect.
    let text = e.objects.str_val(a[0]);
    let name = e.objects.handle(&text);
    let t = e.objects.type_new(name, a[1]);
    e.file_type(t);
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
fn type_of(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let t = e.objects.type_of(a[0]);
    for fresh in e.objects.take_unfiled_types() {
        e.file_type(fresh);
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
fn make_instance(e: &mut Engine, a: &[Obj]) -> EvalResult {
    let tree = e.resolve_tree(a[0]);
    let o = e.objects.instance(tree, 1);
    e.objects.set_data(o, 0, a[1].word());
    Ok(o)
}

/// The type word is written from the operand, whatever it is. Whether that
/// operand is a REGISTERED type is x-lang's question to ask.
fn obj_make(e: &mut Engine, a: &[Obj]) -> EvalResult {
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

pub const TABLE: &[PrimDef] = &[
    PrimDef::both("eq?", "obj", "eq?", 2, eq),
    PrimDef::both("same?", "obj", "same?", 2, same),
    PrimDef::bare("first", 1, first),
    PrimDef::bare("rest", 1, rest),
    PrimDef::bare("pair", 2, pair),
    PrimDef::both_full("make-type", "type", "make", 2, type_make),
    PrimDef::both_full("type-of", "type", "of", 1, type_of),
    PrimDef::both("type?", "type", "?", 2, type_is),
    PrimDef::both_full("make-instance", "type", "make-instance", 2, make_instance),
    PrimDef::filed_full("obj", "make", 2, obj_make),
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
