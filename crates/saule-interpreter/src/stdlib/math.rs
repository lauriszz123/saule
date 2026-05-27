//! Math prelude functions and constants.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use std::thread_local;

use crate::env::Environment;
use crate::stdlib::{expect_arity, expect_min_arity};
use crate::value::{ClassObject, Value};

pub fn install(env: &std::rc::Rc<std::cell::RefCell<Environment>>) {
    let mut static_fields = HashMap::new();
    static_fields.insert("abs".to_string(), native("Math.abs", math_abs));
    static_fields.insert("acos".to_string(), native("Math.acos", math_acos));
    static_fields.insert("asin".to_string(), native("Math.asin", math_asin));
    static_fields.insert("atan".to_string(), native("Math.atan", math_atan));
    static_fields.insert("min".to_string(), native("Math.min", math_min));
    static_fields.insert("max".to_string(), native("Math.max", math_max));
    static_fields.insert("cos".to_string(), native("Math.cos", math_cos));
    static_fields.insert("sin".to_string(), native("Math.sin", math_sin));
    static_fields.insert("tan".to_string(), native("Math.tan", math_tan));
    static_fields.insert("deg".to_string(), native("Math.deg", math_deg));
    static_fields.insert("rad".to_string(), native("Math.rad", math_rad));
    static_fields.insert("exp".to_string(), native("Math.exp", math_exp));
    static_fields.insert("floor".to_string(), native("Math.floor", math_floor));
    static_fields.insert("ceil".to_string(), native("Math.ceil", math_ceil));
    static_fields.insert("fmod".to_string(), native("Math.fmod", math_fmod));
    static_fields.insert("log".to_string(), native("Math.log", math_log));
    static_fields.insert("modf".to_string(), native("Math.modf", math_modf));
    static_fields.insert("random".to_string(), native("Math.random", math_random));
    static_fields.insert(
        "randomseed".to_string(),
        native("Math.randomseed", math_randomseed),
    );
    static_fields.insert(
        "tointeger".to_string(),
        native("Math.tointeger", math_tointeger),
    );
    static_fields.insert("type".to_string(), native("Math.type", math_type));
    static_fields.insert("ult".to_string(), native("Math.ult", math_ult));
    static_fields.insert("round".to_string(), native("Math.round", math_round));
    static_fields.insert("sqrt".to_string(), native("Math.sqrt", math_sqrt));
    static_fields.insert("pow".to_string(), native("Math.pow", math_pow));
    static_fields.insert("clamp".to_string(), native("Math.clamp", math_clamp));
    static_fields.insert("sign".to_string(), native("Math.sign", math_sign));
    static_fields.insert("pi".to_string(), Value::Float(std::f64::consts::PI));
    static_fields.insert("huge".to_string(), Value::Float(f64::INFINITY));
    static_fields.insert("maxinteger".to_string(), Value::Int(i64::MAX));
    static_fields.insert("mininteger".to_string(), Value::Int(i64::MIN));
    static_fields.insert("e".to_string(), Value::Float(std::f64::consts::E));

    let class = ClassObject {
        name: "Math".to_string(),
        parent: None,
        field_defs: Vec::new(),
        methods: HashMap::new(),
        static_fields: RefCell::new(static_fields),
        static_methods: HashMap::new(),
        constructor: None,
    };
    env.borrow_mut()
        .define("Math".to_string(), Value::Class(Rc::new(class)));

}

fn native(name: &'static str, func: fn(&[Value]) -> Result<Value, String>) -> Value {
    Value::Native(Rc::new(crate::value::NativeFn { name, func }))
}

#[derive(Clone, Copy)]
enum Number {
    Int(i64),
    Float(f64),
}

thread_local! {
    static RNG_STATE: RefCell<u64> = const { RefCell::new(0x6a09e667f3bcc909) };
}

fn next_u64() -> u64 {
    RNG_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let mut x = *state;
        if x == 0 {
            x = 0x9e3779b97f4a7c15;
        }
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let out = x.wrapping_mul(0x2545f4914f6cdd1d);
        *state = x;
        out
    })
}

fn random_unit() -> f64 {
    // Use top 53 bits for an IEEE754 fraction in [0, 1).
    let bits = next_u64() >> 11;
    (bits as f64) * (1.0 / ((1u64 << 53) as f64))
}

impl Number {
    fn as_f64(self) -> f64 {
        match self {
            Number::Int(n) => n as f64,
            Number::Float(f) => f,
        }
    }
}

fn number_at(args: &[Value], idx: usize, fn_name: &str) -> Result<Number, String> {
    match args.get(idx) {
        Some(Value::Int(n)) => Ok(Number::Int(*n)),
        Some(Value::Float(f)) => Ok(Number::Float(*f)),
        Some(v) => Err(format!(
            "{fn_name} expects numeric arguments; arg #{} is `{}`",
            idx + 1,
            v.type_name()
        )),
        None => Err(format!("{fn_name} missing arg #{}", idx + 1)),
    }
}

fn math_abs(args: &[Value]) -> Result<Value, String> {
    expect_arity("abs", args, 1)?;
    match number_at(args, 0, "abs")? {
        Number::Int(n) => Ok(Value::Int(n.abs())),
        Number::Float(f) => Ok(Value::Float(f.abs())),
    }
}

fn math_acos(args: &[Value]) -> Result<Value, String> {
    expect_arity("acos", args, 1)?;
    Ok(Value::Float(number_at(args, 0, "acos")?.as_f64().acos()))
}

fn math_asin(args: &[Value]) -> Result<Value, String> {
    expect_arity("asin", args, 1)?;
    Ok(Value::Float(number_at(args, 0, "asin")?.as_f64().asin()))
}

fn math_atan(args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 && args.len() != 2 {
        return Err(format!("atan expects 1 or 2 arguments, got {}", args.len()));
    }
    let y = number_at(args, 0, "atan")?.as_f64();
    if args.len() == 1 {
        Ok(Value::Float(y.atan()))
    } else {
        let x = number_at(args, 1, "atan")?.as_f64();
        Ok(Value::Float(y.atan2(x)))
    }
}

fn math_min(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("min", args, 1)?;
    let mut min = number_at(args, 0, "min")?.as_f64();
    for i in 1..args.len() {
        min = min.min(number_at(args, i, "min")?.as_f64());
    }
    Ok(Value::Float(min))
}

fn math_max(args: &[Value]) -> Result<Value, String> {
    expect_min_arity("max", args, 1)?;
    let mut max = number_at(args, 0, "max")?.as_f64();
    for i in 1..args.len() {
        max = max.max(number_at(args, i, "max")?.as_f64());
    }
    Ok(Value::Float(max))
}

fn math_cos(args: &[Value]) -> Result<Value, String> {
    expect_arity("cos", args, 1)?;
    Ok(Value::Float(number_at(args, 0, "cos")?.as_f64().cos()))
}

fn math_sin(args: &[Value]) -> Result<Value, String> {
    expect_arity("sin", args, 1)?;
    Ok(Value::Float(number_at(args, 0, "sin")?.as_f64().sin()))
}

fn math_tan(args: &[Value]) -> Result<Value, String> {
    expect_arity("tan", args, 1)?;
    Ok(Value::Float(number_at(args, 0, "tan")?.as_f64().tan()))
}

fn math_deg(args: &[Value]) -> Result<Value, String> {
    expect_arity("deg", args, 1)?;
    Ok(Value::Float(
        number_at(args, 0, "deg")?.as_f64() * 180.0 / std::f64::consts::PI,
    ))
}

fn math_rad(args: &[Value]) -> Result<Value, String> {
    expect_arity("rad", args, 1)?;
    Ok(Value::Float(
        number_at(args, 0, "rad")?.as_f64() * std::f64::consts::PI / 180.0,
    ))
}

fn math_exp(args: &[Value]) -> Result<Value, String> {
    expect_arity("exp", args, 1)?;
    Ok(Value::Float(number_at(args, 0, "exp")?.as_f64().exp()))
}

fn math_floor(args: &[Value]) -> Result<Value, String> {
    expect_arity("floor", args, 1)?;
    match number_at(args, 0, "floor")? {
        Number::Int(n) => Ok(Value::Int(n)),
        Number::Float(f) => Ok(Value::Int(f.floor() as i64)),
    }
}

fn math_ceil(args: &[Value]) -> Result<Value, String> {
    expect_arity("ceil", args, 1)?;
    match number_at(args, 0, "ceil")? {
        Number::Int(n) => Ok(Value::Int(n)),
        Number::Float(f) => Ok(Value::Int(f.ceil() as i64)),
    }
}

fn math_fmod(args: &[Value]) -> Result<Value, String> {
    expect_arity("fmod", args, 2)?;
    let a = number_at(args, 0, "fmod")?;
    let b = number_at(args, 1, "fmod")?;
    match (a, b) {
        (Number::Int(x), Number::Int(y)) => {
            if y == 0 {
                return Err("fmod divisor cannot be zero".to_string());
            }
            Ok(Value::Int(x % y))
        }
        _ => {
            let x = a.as_f64();
            let y = b.as_f64();
            if y == 0.0 {
                return Err("fmod divisor cannot be zero".to_string());
            }
            Ok(Value::Float(x % y))
        }
    }
}

fn math_log(args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 && args.len() != 2 {
        return Err(format!("log expects 1 or 2 arguments, got {}", args.len()));
    }
    let x = number_at(args, 0, "log")?.as_f64();
    if x <= 0.0 {
        return Err("log expects a positive value".to_string());
    }
    if args.len() == 1 {
        Ok(Value::Float(x.ln()))
    } else {
        let base = number_at(args, 1, "log")?.as_f64();
        if base <= 0.0 || (base - 1.0).abs() < f64::EPSILON {
            return Err("log base must be positive and not equal to 1".to_string());
        }
        Ok(Value::Float(x.log(base)))
    }
}

fn math_modf(args: &[Value]) -> Result<Value, String> {
    expect_arity("modf", args, 1)?;
    let x = number_at(args, 0, "modf")?.as_f64();
    let int_part = x.trunc();
    let frac_part = x - int_part;
    Ok(Value::Table(Rc::new(RefCell::new(
        crate::value::TableObject::from_array(vec![
            Value::Float(int_part),
            Value::Float(frac_part),
        ]),
    ))))
}

fn math_round(args: &[Value]) -> Result<Value, String> {
    expect_arity("round", args, 1)?;
    match number_at(args, 0, "round")? {
        Number::Int(n) => Ok(Value::Int(n)),
        Number::Float(f) => Ok(Value::Int(f.round() as i64)),
    }
}

fn math_sqrt(args: &[Value]) -> Result<Value, String> {
    expect_arity("sqrt", args, 1)?;
    let x = number_at(args, 0, "sqrt")?.as_f64();
    if x < 0.0 {
        return Err("sqrt expects a non-negative number".to_string());
    }
    Ok(Value::Float(x.sqrt()))
}

fn math_random(args: &[Value]) -> Result<Value, String> {
    match args.len() {
        0 => Ok(Value::Float(random_unit())),
        1 => {
            let n = number_at(args, 0, "random")?;
            let hi = match n {
                Number::Int(i) => i,
                Number::Float(f) => {
                    if f.fract() != 0.0 {
                        return Err("random upper bound must be an integer".to_string());
                    }
                    f as i64
                }
            };
            if hi < 1 {
                return Err("random upper bound must be >= 1".to_string());
            }
            let span = hi as u64;
            Ok(Value::Int((next_u64() % span) as i64 + 1))
        }
        2 => {
            let lo = to_i64(number_at(args, 0, "random")?, "random lower bound")?;
            let hi = to_i64(number_at(args, 1, "random")?, "random upper bound")?;
            if lo > hi {
                return Err("random expects lower bound <= upper bound".to_string());
            }
            let span = (hi as i128 - lo as i128 + 1) as u128;
            let v = (next_u64() as u128) % span;
            Ok(Value::Int(lo + v as i64))
        }
        _ => Err(format!("random expects 0, 1, or 2 arguments, got {}", args.len())),
    }
}

fn math_randomseed(args: &[Value]) -> Result<Value, String> {
    expect_arity("randomseed", args, 1)?;
    let seed = match number_at(args, 0, "randomseed")? {
        Number::Int(i) => i as u64,
        Number::Float(f) => f.to_bits(),
    };
    RNG_STATE.with(|cell| *cell.borrow_mut() = if seed == 0 { 1 } else { seed });
    Ok(Value::Nil)
}

fn math_tointeger(args: &[Value]) -> Result<Value, String> {
    expect_arity("tointeger", args, 1)?;
    let out = match &args[0] {
        Value::Int(i) => Some(*i),
        Value::Float(f) => {
            if f.is_finite() && f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64
            {
                Some(*f as i64)
            } else {
                None
            }
        }
        _ => None,
    };
    Ok(out.map(Value::Int).unwrap_or(Value::Nil))
}

fn math_type(args: &[Value]) -> Result<Value, String> {
    expect_arity("type", args, 1)?;
    match args[0] {
        Value::Int(_) => Ok(Value::Str(Rc::new("integer".to_string()))),
        Value::Float(_) => Ok(Value::Str(Rc::new("float".to_string()))),
        _ => Ok(Value::Nil),
    }
}

fn math_ult(args: &[Value]) -> Result<Value, String> {
    expect_arity("ult", args, 2)?;
    let a = to_i64(number_at(args, 0, "ult")?, "ult arg #1")? as u64;
    let b = to_i64(number_at(args, 1, "ult")?, "ult arg #2")? as u64;
    Ok(Value::Bool(a < b))
}

fn math_pow(args: &[Value]) -> Result<Value, String> {
    expect_arity("pow", args, 2)?;
    let base = number_at(args, 0, "pow")?.as_f64();
    let exp = number_at(args, 1, "pow")?.as_f64();
    Ok(Value::Float(base.powf(exp)))
}

fn math_clamp(args: &[Value]) -> Result<Value, String> {
    expect_arity("clamp", args, 3)?;
    let value = number_at(args, 0, "clamp")?.as_f64();
    let min = number_at(args, 1, "clamp")?.as_f64();
    let max = number_at(args, 2, "clamp")?.as_f64();
    if min > max {
        return Err("clamp expects min <= max".to_string());
    }
    Ok(Value::Float(value.clamp(min, max)))
}

fn math_sign(args: &[Value]) -> Result<Value, String> {
    expect_arity("sign", args, 1)?;
    let x = number_at(args, 0, "sign")?.as_f64();
    let out = if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    };
    Ok(Value::Int(out))
}

fn to_i64(n: Number, label: &str) -> Result<i64, String> {
    match n {
        Number::Int(i) => Ok(i),
        Number::Float(f) => {
            if f.fract() != 0.0 || !f.is_finite() || f < i64::MIN as f64 || f > i64::MAX as f64 {
                Err(format!("{label} must be an integer"))
            } else {
                Ok(f as i64)
            }
        }
    }
}


