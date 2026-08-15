#[test]
fn value_is_still_two_words() {
    // `VM_DESIGN.md` §4.1 measures `Value` at 16 bytes and argues from that
    // number that NaN-boxing is not worth it: no variant payload exceeds one
    // word, so the enum is a payload word plus a tag word.
    //
    // `Value::VmFunction` must not break that. An `Rc<dyn Trait>` is a *fat*
    // pointer — two words — which would push the whole enum to 24 bytes and
    // silently inflate every register, every table slot and every argument
    // in the language.
    assert_eq!(std::mem::size_of::<saule_interpreter::Value>(), 16);
    assert_eq!(std::mem::size_of::<Option<saule_interpreter::Value>>(), 16);
}
