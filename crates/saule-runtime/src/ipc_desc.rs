//! What the runtime needs to know about an `extern interface` to call it
//! across processes, or to answer calls to it.
//!
//! Types are erased when a program runs, but a call to another process has
//! to know them: an argument declared `WindowEvents` must go as a reference
//! to an object, one declared `KeyEvent` as a copy of its fields, and what
//! comes back has to be rebuilt as the same kinds of values. So the compiler
//! writes a [`Bundle`] — the interface, every `extern interface` and data
//! class reachable from its signatures, and their types — into the program
//! as a string constant, and the runtime reads it back the first time the
//! interface is connected to or published.
//!
//! A string rather than a new kind of constant so that `saule build`'s
//! bytecode format carries it unchanged.
//!
//! ## Format
//!
//! One item per line, fields separated by single spaces:
//!
//! ```text
//! iface Display
//! method createWindow call s @WindowEvents; -> @Window;
//! method commit async ->
//! data KeyEvent
//! field key s
//! ```
//!
//! The first `iface` line is the root. A type is written in prefix form with
//! no spaces: `n` nil, `b` boolean, `i` integer, `f` float, `s` string,
//! `y` Bytes, `*` any, `?T` nullable, `[T` an array table, `{KT` a keyed
//! table, `@Name;` an extern interface, `#Name;` a data class.

/// The type of one value crossing a process boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Nil,
    Bool,
    Int,
    Float,
    Str,
    Bytes,
    Any,
    Opt(Box<Ty>),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    /// An `extern interface`: passed by reference.
    Iface(String),
    /// A plain data class: copied.
    Data(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodDesc {
    pub name: String,
    pub is_async: bool,
    pub params: Vec<Ty>,
    pub returns: Vec<Ty>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfaceDesc {
    pub name: String,
    pub methods: Vec<MethodDesc>,
}

impl IfaceDesc {
    pub fn method(&self, name: &str) -> Option<&MethodDesc> {
        self.methods.iter().find(|m| m.name == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataDesc {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

/// One interface and everything its signatures reach.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bundle {
    pub ifaces: Vec<IfaceDesc>,
    pub data: Vec<DataDesc>,
}

impl Bundle {
    /// The interface the bundle was written for.
    pub fn root(&self) -> Option<&IfaceDesc> {
        self.ifaces.first()
    }

    pub fn encode(&self) -> String {
        let mut out = String::new();
        for i in &self.ifaces {
            out.push_str("iface ");
            out.push_str(&i.name);
            out.push('\n');
            for m in &i.methods {
                out.push_str("method ");
                out.push_str(&m.name);
                out.push_str(if m.is_async { " async" } else { " call" });
                for p in &m.params {
                    out.push(' ');
                    p.encode(&mut out);
                }
                out.push_str(" ->");
                for r in &m.returns {
                    out.push(' ');
                    r.encode(&mut out);
                }
                out.push('\n');
            }
        }
        for d in &self.data {
            out.push_str("data ");
            out.push_str(&d.name);
            out.push('\n');
            for (name, ty) in &d.fields {
                out.push_str("field ");
                out.push_str(name);
                out.push(' ');
                ty.encode(&mut out);
                out.push('\n');
            }
        }
        out
    }

    pub fn parse(text: &str) -> Result<Bundle, String> {
        let mut bundle = Bundle::default();
        for (n, line) in text.lines().enumerate() {
            let bad = |why: &str| format!("interface description, line {}: {why}", n + 1);
            let mut words = line.split(' ');
            match words.next() {
                Some("iface") => bundle.ifaces.push(IfaceDesc {
                    name: words.next().ok_or_else(|| bad("no name"))?.to_string(),
                    methods: Vec::new(),
                }),
                Some("method") => {
                    let name = words.next().ok_or_else(|| bad("no method name"))?.to_string();
                    let is_async = match words.next() {
                        Some("async") => true,
                        Some("call") => false,
                        _ => return Err(bad("no call kind")),
                    };
                    let mut params = Vec::new();
                    let mut returns = Vec::new();
                    let mut after_arrow = false;
                    for w in words {
                        if w == "->" {
                            after_arrow = true;
                            continue;
                        }
                        let ty = Ty::parse(w).map_err(|e| bad(&e))?;
                        if after_arrow {
                            returns.push(ty);
                        } else {
                            params.push(ty);
                        }
                    }
                    bundle
                        .ifaces
                        .last_mut()
                        .ok_or_else(|| bad("a method outside an interface"))?
                        .methods
                        .push(MethodDesc {
                            name,
                            is_async,
                            params,
                            returns,
                        });
                }
                Some("data") => bundle.data.push(DataDesc {
                    name: words.next().ok_or_else(|| bad("no name"))?.to_string(),
                    fields: Vec::new(),
                }),
                Some("field") => {
                    let name = words.next().ok_or_else(|| bad("no field name"))?.to_string();
                    let ty = Ty::parse(words.next().ok_or_else(|| bad("no field type"))?)
                        .map_err(|e| bad(&e))?;
                    bundle
                        .data
                        .last_mut()
                        .ok_or_else(|| bad("a field outside a class"))?
                        .fields
                        .push((name, ty));
                }
                Some("") | None => {}
                Some(other) => return Err(bad(&format!("unknown item `{other}`"))),
            }
        }
        if bundle.ifaces.is_empty() {
            return Err("interface description names no interface".into());
        }
        Ok(bundle)
    }
}

impl Ty {
    pub fn encode(&self, out: &mut String) {
        match self {
            Ty::Nil => out.push('n'),
            Ty::Bool => out.push('b'),
            Ty::Int => out.push('i'),
            Ty::Float => out.push('f'),
            Ty::Str => out.push('s'),
            Ty::Bytes => out.push('y'),
            Ty::Any => out.push('*'),
            Ty::Opt(t) => {
                out.push('?');
                t.encode(out);
            }
            Ty::List(t) => {
                out.push('[');
                t.encode(out);
            }
            Ty::Map(k, v) => {
                out.push('{');
                k.encode(out);
                v.encode(out);
            }
            Ty::Iface(n) => {
                out.push('@');
                out.push_str(n);
                out.push(';');
            }
            Ty::Data(n) => {
                out.push('#');
                out.push_str(n);
                out.push(';');
            }
        }
    }

    pub fn parse(text: &str) -> Result<Ty, String> {
        let (ty, rest) = Ty::parse_prefix(text)?;
        if !rest.is_empty() {
            return Err(format!("trailing `{rest}` after a type"));
        }
        Ok(ty)
    }

    fn parse_prefix(text: &str) -> Result<(Ty, &str), String> {
        let mut chars = text.chars();
        let c = chars.next().ok_or("a type is missing")?;
        let rest = chars.as_str();
        Ok(match c {
            'n' => (Ty::Nil, rest),
            'b' => (Ty::Bool, rest),
            'i' => (Ty::Int, rest),
            'f' => (Ty::Float, rest),
            's' => (Ty::Str, rest),
            'y' => (Ty::Bytes, rest),
            '*' => (Ty::Any, rest),
            '?' => {
                let (t, rest) = Ty::parse_prefix(rest)?;
                (Ty::Opt(Box::new(t)), rest)
            }
            '[' => {
                let (t, rest) = Ty::parse_prefix(rest)?;
                (Ty::List(Box::new(t)), rest)
            }
            '{' => {
                let (k, rest) = Ty::parse_prefix(rest)?;
                let (v, rest) = Ty::parse_prefix(rest)?;
                (Ty::Map(Box::new(k), Box::new(v)), rest)
            }
            '@' | '#' => {
                let end = rest.find(';').ok_or("a type name without its `;`")?;
                let name = rest[..end].to_string();
                let ty = if c == '@' {
                    Ty::Iface(name)
                } else {
                    Ty::Data(name)
                };
                (ty, &rest[end + 1..])
            }
            other => return Err(format!("unknown type `{other}`")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_reads_back_as_written() {
        let bundle = Bundle {
            ifaces: vec![
                IfaceDesc {
                    name: "Display".into(),
                    methods: vec![
                        MethodDesc {
                            name: "createWindow".into(),
                            is_async: false,
                            params: vec![Ty::Str, Ty::Iface("WindowEvents".into())],
                            returns: vec![Ty::Iface("Window".into())],
                        },
                        MethodDesc {
                            name: "size".into(),
                            is_async: false,
                            params: vec![],
                            returns: vec![Ty::Int, Ty::Int],
                        },
                        MethodDesc {
                            name: "ping".into(),
                            is_async: true,
                            params: vec![],
                            returns: vec![],
                        },
                    ],
                },
                IfaceDesc {
                    name: "WindowEvents".into(),
                    methods: vec![MethodDesc {
                        name: "key".into(),
                        is_async: true,
                        params: vec![Ty::Data("KeyEvent".into())],
                        returns: vec![],
                    }],
                },
            ],
            data: vec![DataDesc {
                name: "KeyEvent".into(),
                fields: vec![
                    ("key".into(), Ty::Str),
                    ("text".into(), Ty::Opt(Box::new(Ty::Str))),
                    (
                        "mods".into(),
                        Ty::Map(Box::new(Ty::Str), Box::new(Ty::List(Box::new(Ty::Bool)))),
                    ),
                    ("any".into(), Ty::Any),
                    ("raw".into(), Ty::Bytes),
                ],
            }],
        };
        let text = bundle.encode();
        assert!(text.starts_with("iface Display\nmethod createWindow call s @WindowEvents; -> @Window;\n"));
        assert_eq!(Bundle::parse(&text).unwrap(), bundle);
        assert_eq!(Bundle::parse(&text).unwrap().root().unwrap().name, "Display");
    }

    #[test]
    fn a_damaged_description_is_an_error_not_a_guess() {
        assert!(Bundle::parse("").is_err());
        assert!(Bundle::parse("method x call ->").is_err());
        assert!(Bundle::parse("iface A\nmethod x maybe ->").is_err());
        assert!(Bundle::parse("iface A\nmethod x call @Foo ->").is_err());
        assert!(Bundle::parse("iface A\nmethod x call q ->").is_err());
        assert!(Bundle::parse("iface A\nfield x s").is_err());
    }
}
