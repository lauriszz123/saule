//! `Assignable<T>` — the write-target abstraction.

use crate::harness::*;

// ── Assignable<T> ─────────────────────────────────────────────────────────

/// A class that opts into `Assignable<string>`, for the tests below.
const TEXT_CLASS: &str = "class Text implements Assignable<string>\n\
                          \x20 local raw: string\n\
                          \x20 fn init(raw: string)\n\
                          \x20   self.raw = raw\n\
                          \x20 end\n\
                          \x20 static fn of(s: string) -> Text\n\
                          \x20   return Text(s)\n\
                          \x20 end\n\
                          \x20 fn value() -> string\n\
                          \x20   return self.raw\n\
                          \x20 end\n\
                          end\n";

#[test]
fn an_annotated_local_builds_the_assignable_class() {
    // The shape the feature exists for: a bare `string` in a slot declared
    // as `Text` becomes a `Text`, so the method call finds an instance.
    must_agree(&format!("{TEXT_CLASS}local t: Text = \"hi\"\nt.value()"));
}

#[test]
fn an_assignable_parameter_is_converted_in_the_callee() {
    // The second of `coerce.rs`'s two sites. The conversion is emitted
    // **after** the default entry stubs, which is what makes it run however
    // the frame was entered — a copy at pc 0 would be jumped straight over
    // by any call that lands on a stub.
    must_agree(&format!(
        "{TEXT_CLASS}fn shout(t: Text) -> string\n\
         \x20 return t.value() .. \"!\"\n\
         end\n\
         shout(\"quiet\")"
    ));
}

#[test]
fn an_assignable_parameter_converts_behind_a_default() {
    // The interaction the placement is about: `pad` has a default, so the
    // proto has entry stubs, and a one-argument call enters at a stub rather
    // than at the body. The conversion must still happen.
    must_agree(&format!(
        "{TEXT_CLASS}fn tag(t: Text, pad: integer = 2) -> string\n\
         \x20 return t.value() .. tostring(pad)\n\
         end\n\
         tag(\"a\") .. \"/\" .. tag(\"b\", 9)"
    ));
}

#[test]
fn an_assignable_slot_leaves_nil_and_an_instance_alone() {
    // The two runtime checks the emitted sequence makes, both of which
    // `to_declared` also makes: `nil` fills a nullable slot on its own
    // terms, and a value that is already an instance is returned untouched
    // rather than passed through `of` a second time.
    must_agree(&format!(
        "{TEXT_CLASS}local a: Text? = nil\n\
         local b: Text = Text(\"direct\")\n\
         local c: Text? = \"present\"\n\
         tostring(a == nil) .. b.value() .. c!.value()"
    ));
}

#[test]
fn a_shadowed_assignable_class_does_not_coerce() {
    // Trap 1 again, at the binding site. A module-level `local Text = {…}`
    // is a module slot, and the class of the same name must not fire its
    // `of` on a slot annotated with that local's type.
    must_agree(&format!(
        "{TEXT_CLASS}local Text = 1\n\
         local n: integer = 2\n\
         Text + n"
    ));
}

#[test]
fn a_compound_assignment_evaluates_its_target_once() {
    // **A miscompile that predates this work and that nothing could see.**
    // `t[idx()] += 1` called `idx` twice under the VM and once under the
    // tree-walker: the compiler built a synthetic `target op value` node
    // holding a *clone* of the target, then assigned to the target again, so
    // every sub-expression of it ran twice. Wrong value, exit status 0.
    //
    // `SAULE_DIFF=1` could not catch it: the only fixture that writes this
    // shape, `tests/compound_assign.sau`, also compound-assigns to a member
    // two lines later, which refused and sent the whole file to the oracle.
    // A refusal standing next to a miscompile is trap 3.
    //
    // The compiler now refuses a target it cannot evaluate once, so the
    // module falls back to the engine that gets it right. Both halves are
    // asserted: that it refuses rather than compiling the double
    // evaluation, and that the oracle's answer is the `1` a correct
    // compilation would have to produce.
    let src = "local calls: integer = 0\n\
               local t: table<integer> = {10, 20}\n\
               fn idx() -> integer\n\
               \x20 calls += 1\n\
               \x20 return 1\n\
               end\n\
               t[idx()] += 1\n\
               calls .. \":\" .. t[1]";
    let module = front_end(src);
    assert!(
        matches!(
            saule_vm::compile(&module, "x.sau", src),
            Err(saule_vm::CompileError::Unsupported { .. })
        ),
        "a target with a side-effecting subscript must refuse, not be evaluated twice"
    );
    assert_eq!(tree_walker(&module), Outcome::Value("string:1:11".into()));
}

#[test]
fn a_compound_assignment_to_a_simple_member_compiles() {
    // The other side of the same rule: `self` and a bare name are re-read
    // for free, so these are compiled rather than refused. Asserted to
    // *compile*, not merely to agree — agreeing is what a fallback does too.
    let src = "class Counter\n\
               \x20 n: integer\n\
               \x20 static total: integer = 0\n\
               \x20 fn init()\n\
               \x20   self.n = 0\n\
               \x20 end\n\
               \x20 fn bump()\n\
               \x20   self.n += 3\n\
               \x20   Counter.total += 1\n\
               \x20 end\n\
               end\n\
               local c: Counter = Counter()\n\
               c.bump()\n\
               c.bump()\n\
               c.n .. \":\" .. Counter.total";
    must_agree(src);
    let module = front_end(src);
    assert!(
        saule_vm::compile(&module, "x.sau", src).is_ok(),
        "a compound assignment to `self.f` and `Class.f` must compile, not fall back"
    );
}



