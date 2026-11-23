//! `Iface.connect(name)` and `Iface.publish(name, service)` on an
//! `extern interface`.
//!
//! Both compile to a call of the runtime's internal `Ipc` class, with one
//! extra first argument: a description of the interface — its methods, which
//! are `async`, and the types they take and return, along with every other
//! extern interface and data class those types reach. Types are erased at run
//! time, and this is what lets the runtime send a `WindowEvents` by
//! reference and a `KeyEvent` by copy (see `saule_runtime::ipc_desc`).

use std::collections::{BTreeSet, VecDeque};

use saule_ast::{Spanned, Type};
use saule_runtime::Value;
use saule_runtime::ipc_desc::{Bundle, DataDesc, IfaceDesc, MethodDesc, Ty};
use saule_runtime::value::SauleStr;

use super::super::ctx::Compiler;
use super::results::{Results, Want};
use super::CompileError;
use crate::op::{Instruction, Op};

impl Compiler<'_> {
    /// `iface.method(args)` where `iface` names an extern interface and
    /// `method` is one of its implicit statics, compiled; `None` when this is
    /// not such a call.
    pub(crate) fn extern_static_call(
        &mut self,
        obj: &Spanned<saule_ast::Expr>,
        name: &str,
        args: &[&Spanned<saule_ast::Expr>],
        dst: u16,
        want: Want,
        span: &std::ops::Range<usize>,
    ) -> Option<Result<Results, CompileError>> {
        let saule_ast::Expr::Ident(iface) = &obj.value else {
            return None;
        };
        if !matches!(name, "connect" | "publish")
            || !self.not_shadowed(iface)
            || !saule_semantic::is_extern_interface(iface)
        {
            return None;
        }
        Some(self.emit_extern_static(iface, name, args, dst, want, span))
    }

    fn emit_extern_static(
        &mut self,
        iface: &str,
        name: &str,
        args: &[&Spanned<saule_ast::Expr>],
        dst: u16,
        want: Want,
        span: &std::ops::Range<usize>,
    ) -> Result<Results, CompileError> {
        let description = describe(iface, span)?;
        let native = match self.prelude_value("Ipc") {
            Some(Value::Class(c)) => c.lookup_static_field(name),
            _ => None,
        }
        .ok_or_else(|| CompileError::ExternInterface {
            message: format!("`{iface}.{name}` needs inter-process calls, which this platform lacks"),
            help: "extern interfaces run on Linux, macOS and Windows, not in a browser".into(),
            span: span.clone(),
        })?;

        let k = self.constant(native, span)?;
        let kd = self.constant(Value::Str(SauleStr::new(description)), span)?;
        let m = self.mark();
        // `CALLNAT` reads its arguments from `A+1..`: the description first,
        // then what the program passed.
        let n = args.len() as u16 + 1;
        let base = self.alloc_n((n + 1).max(want.slots()), span)?;
        let rd = self.reg8(base + 1, span)?;
        self.emit(Instruction::abx(Op::LOADK, rd, kd), span);
        for (i, arg) in args.iter().enumerate() {
            self.expr_to(arg, base + 2 + i as u16)?;
        }
        let a = self.reg8(base, span)?;
        self.emit(
            Instruction::abc(Op::CALLNAT, a, n as u8 + 1, want.c()),
            span,
        );
        self.emit(Instruction::ax_of(Op::EXTRAARG, k as u32), span);
        self.finish_call(base, dst, want, m, span)
    }
}

/// The description of `root` and everything its signatures reach, written
/// out for the runtime.
fn describe(root: &str, span: &std::ops::Range<usize>) -> Result<String, CompileError> {
    let mut bundle = Bundle::default();
    let mut ifaces = VecDeque::from([root.to_string()]);
    let mut seen_ifaces = BTreeSet::new();
    let mut data = VecDeque::new();
    let mut seen_data = BTreeSet::new();

    while let Some(name) = ifaces.pop_front() {
        if !seen_ifaces.insert(name.clone()) {
            continue;
        }
        let mut desc = IfaceDesc {
            name: name.clone(),
            methods: Vec::new(),
        };
        for method in saule_semantic::interface_required_methods(&name) {
            let Some(sig) = saule_semantic::lookup_interface_method(&name, &method) else {
                continue;
            };
            let mut ty = |t: &Type| ty_of(t, &name, &method, &mut ifaces, &mut data, span);
            let params = sig.params.iter().map(|p| ty(&p.ty)).collect::<Result<_, _>>()?;
            let returns = match &sig.return_ty {
                None => Vec::new(),
                Some(Type::Tuple(items)) => items.iter().map(&mut ty).collect::<Result<_, _>>()?,
                Some(t) => vec![ty(t)?],
            };
            desc.methods.push(MethodDesc {
                name: method,
                is_async: sig.remote == saule_semantic::Remote::Async,
                params,
                returns,
            });
        }
        bundle.ifaces.push(desc);
    }

    while let Some(name) = data.pop_front() {
        if !seen_data.insert(name.clone()) {
            continue;
        }
        let fields = saule_semantic::with_classes(|r| {
            r.get(&name).map(|c| {
                let mut f: Vec<(String, Type)> = c
                    .field_types
                    .iter()
                    .filter(|(n, _)| !c.static_fields.contains(*n))
                    .map(|(n, t)| (n.clone(), t.clone()))
                    .collect();
                f.sort_by(|a, b| a.0.cmp(&b.0));
                f
            })
        })
        .unwrap_or_default();
        let mut desc = DataDesc {
            name: name.clone(),
            fields: Vec::new(),
        };
        for (field, t) in fields {
            let ty = ty_of(&t, &name, &field, &mut ifaces, &mut data, span)?;
            desc.fields.push((field, ty));
        }
        bundle.data.push(desc);
    }
    // A data class's fields can name further interfaces; describe those
    // too. (Rare enough that one more pass is the whole loop it needs.)
    while let Some(name) = ifaces.pop_front() {
        if seen_ifaces.contains(&name) {
            continue;
        }
        let rest = describe(&name, span)?;
        let more = Bundle::parse(&rest).map_err(|message| CompileError::ExternInterface {
            message,
            help: String::new(),
            span: span.clone(),
        })?;
        for i in more.ifaces {
            if seen_ifaces.insert(i.name.clone()) {
                bundle.ifaces.push(i);
            }
        }
        for d in more.data {
            if seen_data.insert(d.name.clone()) {
                bundle.data.push(d);
            }
        }
    }
    Ok(bundle.encode())
}

/// The runtime's view of a declared type.
fn ty_of(
    t: &Type,
    owner: &str,
    member: &str,
    ifaces: &mut VecDeque<String>,
    data: &mut VecDeque<String>,
    span: &std::ops::Range<usize>,
) -> Result<Ty, CompileError> {
    Ok(match t {
        Type::Nullable(inner) => Ty::Opt(Box::new(ty_of(inner, owner, member, ifaces, data, span)?)),
        Type::Table { key: None, value } => {
            Ty::List(Box::new(ty_of(value, owner, member, ifaces, data, span)?))
        }
        Type::Table {
            key: Some(k),
            value,
        } => Ty::Map(
            Box::new(ty_of(k, owner, member, ifaces, data, span)?),
            Box::new(ty_of(value, owner, member, ifaces, data, span)?),
        ),
        Type::Named(n) => match n.as_str() {
            "nil" => Ty::Nil,
            "boolean" => Ty::Bool,
            "integer" | "byte" => Ty::Int,
            "float" => Ty::Float,
            "string" => Ty::Str,
            "bytes" => Ty::Bytes,
            "any" => Ty::Any,
            _ if saule_semantic::is_extern_interface(n) => {
                ifaces.push_back(n.clone());
                Ty::Iface(n.clone())
            }
            _ if saule_semantic::with_classes(|r| r.contains_key(n)) => {
                data.push_back(n.clone());
                Ty::Data(n.clone())
            }
            _ => {
                return Err(CompileError::ExternInterface {
                    message: format!(
                        "`{owner}.{member}` uses `{n}`, which this module cannot see"
                    ),
                    help: format!(
                        "import `{n}` too, from the module that declares `{owner}` — a call to \
                         another process has to know every type it sends"
                    ),
                    span: span.clone(),
                });
            }
        },
        // The checker refuses anything else in an extern interface.
        _ => Ty::Any,
    })
}
