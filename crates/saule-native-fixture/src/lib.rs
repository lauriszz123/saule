//! `fixture` — a native package that exercises every kind of export
//! `saule-sdk` offers, for the test suites to load and drive from Saule.
//!
//! Each item is here to prove one thing works across the boundary; the
//! doc comment says which. Nothing is meant to be useful on its own.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use saule_sdk::prelude::*;

saule_package! {
    name = "fixture",
    version = "1.2.3",
    doc = "Exercises every kind of export the SDK has.",
    classes {
        Util = "Free functions over every mapped type.",
    }
}

thread_local! {
    /// Counters alive in the package right now — how the tests see that
    /// dropping the last Saule reference runs the destructor.
    static LIVE: Cell<i64> = const { Cell::new(0) };
    /// A counter the package keeps for itself and also hands out.
    static SHARED: RefCell<Option<SObject<Counter>>> = const { RefCell::new(None) };
}

/// A counter that starts somewhere and counts in steps.
#[saule_class]
pub struct Counter {
    value: i64,
    step: i64,
}

impl Counter {
    fn make(value: i64) -> Self {
        LIVE.with(|l| l.set(l.get() + 1));
        Counter { value, step: 1 }
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        LIVE.with(|l| l.set(l.get() - 1));
    }
}

#[saule_methods]
impl Counter {
    /// A counter at `start`. Called as `Counter(start)`.
    pub fn new(start: i64) -> Self {
        Counter::make(start)
    }

    /// Add the step, and return the new value.
    pub fn bump(&mut self) -> i64 {
        self.value += self.step;
        self.value
    }

    /// The current value.
    #[saule(getter)]
    pub fn value(&self) -> i64 {
        self.value
    }

    /// How much `bump` adds.
    #[saule(getter)]
    pub fn step(&self) -> i64 {
        self.step
    }

    #[saule(setter)]
    pub fn set_step(&mut self, step: i64) {
        self.step = step;
    }

    /// A new counter with this one's value.
    pub fn fork(&self) -> Counter {
        Counter::make(self.value)
    }

    /// Add `other`'s value into this counter. Borrows two objects at once.
    pub fn absorb(&mut self, other: &Counter) -> i64 {
        self.value += other.value;
        self.value
    }

    /// Call `f`, which may try to use this counter again while it is
    /// mutably borrowed here.
    #[saule(sig(f = "fn() -> nil"))]
    pub fn while_bumping(&mut self, f: SFunction) -> Result<i64, String> {
        self.value += 1;
        f.call(&[])?;
        Ok(self.value)
    }

    /// How many counters exist in the package right now.
    pub fn live() -> i64 {
        LIVE.with(Cell::get)
    }

    /// A counter from text, or an error.
    pub fn parse(text: &str) -> Result<Counter, String> {
        text.trim()
            .parse::<i64>()
            .map(Counter::make)
            .map_err(|_| format!("`{text}` is not a number"))
    }

    /// Not exported: not `pub`.
    fn helper(&self) -> i64 {
        self.value * 2
    }

    /// Exported under another name, using a private helper.
    #[saule(name = "doubled")]
    pub fn twice(&self) -> i64 {
        self.helper()
    }
}

/// Which way to round.
#[saule_enum]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rounding {
    /// Toward negative infinity.
    Down,
    /// Toward positive infinity.
    Up,
    /// To the nearest integer.
    Nearest,
}

/// Round `x` the given way.
#[saule_export(class = "Util")]
fn round(x: f64, mode: Rounding) -> i64 {
    match mode {
        Rounding::Down => x.floor() as i64,
        Rounding::Up => x.ceil() as i64,
        Rounding::Nearest => x.round() as i64,
    }
}

/// Every rounding mode — enums inside a table.
#[saule_export(class = "Util")]
fn modes() -> Vec<Rounding> {
    vec![Rounding::Down, Rounding::Up, Rounding::Nearest]
}

/// An enum or nothing.
#[saule_export(class = "Util")]
fn preferred(nearest: bool) -> Option<Rounding> {
    nearest.then_some(Rounding::Nearest)
}

/// A table in, copied into a `Vec`.
#[saule_export(class = "Util")]
fn sum(xs: Vec<i64>) -> i64 {
    xs.iter().sum()
}

/// A map out.
#[saule_export(class = "Util")]
fn tally(words: Vec<String>) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    for w in words {
        *out.entry(w).or_insert(0) += 1;
    }
    out
}

/// A narrow integer: out-of-range arguments are errors.
#[saule_export(class = "Util")]
fn narrow(x: i32) -> i32 {
    x
}

/// Two values back, or an error.
#[saule_export(class = "Util")]
fn divmod(a: i64, b: i64) -> Result<(i64, i64), String> {
    if b == 0 {
        return Err("division by zero".to_string());
    }
    Ok((a.div_euclid(b), a.rem_euclid(b)))
}

/// Panics; the interpreter must report it rather than abort.
#[saule_export(class = "Util")]
fn boom() -> i64 {
    panic!("kaboom")
}

/// The same counter every time: one the package also keeps.
#[saule_export(class = "Util")]
fn shared() -> SObject<Counter> {
    SHARED.with(|s| {
        s.borrow_mut()
            .get_or_insert_with(|| SObject::new(Counter::make(100)))
            .clone()
    })
}

/// A table of objects in.
#[saule_export(class = "Util")]
fn total(counters: Vec<SObject<Counter>>) -> Result<i64, String> {
    let mut sum = 0;
    for c in counters {
        sum += c.borrow()?.value;
    }
    Ok(sum)
}

/// Optional borrowed parameters.
#[saule_export(class = "Util")]
fn greet(name: &str, punct: Option<&str>) -> String {
    format!("hello, {name}{}", punct.unwrap_or("."))
}

/// An optional object.
#[saule_export(class = "Util")]
fn value_or_zero(counter: Option<&Counter>) -> i64 {
    counter.map_or(0, |c| c.value)
}

/// A callback that receives an object.
#[saule_export(class = "Util", sig(f = "fn(Counter) -> integer"))]
fn apply(counter: SObject<Counter>, f: SFunction) -> Result<i64, String> {
    match f.call(&[counter.into()])? {
        SValue::Int(i) => Ok(i),
        other => Err(format!("the callback returned {other:?}, not an integer")),
    }
}
