//! Rust types → Saule types, and how a generated shim turns an argument into
//! the value the Rust function takes.

use syn::{GenericArgument, PathArguments, Type};

/// What the mapping needs to know about where it is.
pub(crate) struct TypeCtx<'a> {
    /// Generic type-variable names (`T`, `U`, …) referenced anywhere in the
    /// signature, in first-seen order. Non-empty means a `fn<T, …>` prefix.
    pub(crate) tvars: Vec<String>,
    /// Inside `#[saule_methods]`: the type `Self` stands for. The generated
    /// shims live outside the `impl`, where `Self` means nothing, so every
    /// `Self` is replaced by this.
    pub(crate) self_ty: Option<&'a Type>,
}

impl<'a> TypeCtx<'a> {
    pub(crate) fn new(self_ty: Option<&'a Type>) -> Self {
        TypeCtx {
            tvars: Vec::new(),
            self_ty,
        }
    }

    fn self_name(&self) -> Option<String> {
        self.self_ty.and_then(named_class)
    }
}

/// How the shim produces an argument.
pub(crate) enum Decode {
    /// `<Ty as FromSaule>::from_saule(…)`, passed as-is.
    Owned(Type),
    /// A `String`, passed as `&s` — for `&str` / `&String` parameters.
    StrRef,
    /// `Option<String>`, passed as `.as_deref()` — for `Option<&str>`.
    OptStrRef,
    /// An object of a `#[saule_class]`, borrowed for the call: decoded as
    /// `SObject<Inner>` and passed as `&Inner` / `&mut Inner`. The borrow
    /// is checked, so a re-entrant call that would alias a `&mut` fails with
    /// a Saule error rather than undefined behaviour.
    ObjRef { inner: Type, mutable: bool },
    /// `Option<&Inner>` / `Option<&mut Inner>`.
    OptObjRef { inner: Type, mutable: bool },
}

/// Map a parameter's Rust type. Returns the Saule type, whether the
/// parameter is optional (an `Option<_>`, which may be omitted), and how to
/// decode it.
///
/// `fn_sig` is the signature declared for this slot in `sig(...)` — the only
/// way an `SFunction` can be typed.
pub(crate) fn param(
    ty: &Type,
    ctx: &mut TypeCtx<'_>,
    fn_sig: Option<&str>,
) -> syn::Result<(String, bool, Decode)> {
    // References: `&str`, and borrowed objects.
    if let Type::Reference(r) = ty {
        if is_str_like(&r.elem) {
            return Ok(("string".to_string(), false, Decode::StrRef));
        }
        let inner = resolve_self(&r.elem, ctx)?;
        let name = named_class(&inner).ok_or_else(|| {
            syn::Error::new_spanned(
                ty,
                "only `&str` and references to a `#[saule_class]` type can be \
                 parameters; take this type by value",
            )
        })?;
        return Ok((
            name,
            false,
            Decode::ObjRef {
                inner,
                mutable: r.mutability.is_some(),
            },
        ));
    }

    // `Option<&str>` / `Option<&T>` need a decode of their own; any other
    // `Option<_>` decodes as a whole through `FromSaule`.
    if let Some(inner) = generic_arg(ty, "Option")
        && let Type::Reference(r) = inner
    {
        if is_str_like(&r.elem) {
            return Ok(("string?".to_string(), true, Decode::OptStrRef));
        }
        let class = resolve_self(&r.elem, ctx)?;
        let name = named_class(&class).ok_or_else(|| {
            syn::Error::new_spanned(
                ty,
                "only `Option<&str>` and `Option<&T>` of a `#[saule_class]` \
                 type can be optional reference parameters",
            )
        })?;
        return Ok((
            format!("{name}?"),
            true,
            Decode::OptObjRef {
                inner: class,
                mutable: r.mutability.is_some(),
            },
        ));
    }

    let concrete = resolve_self(ty, ctx)?;
    let (saule, optional) = saule_type(&concrete, ctx, fn_sig)?;
    Ok((saule, optional, Decode::Owned(concrete)))
}

/// Map a return type. Unlike a parameter, a non-unit tuple `(A, B, …)` is
/// accepted and renders as a Saule multi-return `(a, b, …)`.
pub(crate) fn return_type(
    ty: &Type,
    ctx: &mut TypeCtx<'_>,
    fn_sig: Option<&str>,
) -> syn::Result<String> {
    if let Type::Tuple(t) = ty {
        if t.elems.is_empty() {
            return Ok("nil".to_string());
        }
        let mut parts = Vec::with_capacity(t.elems.len());
        for elem in &t.elems {
            let elem = resolve_self(elem, ctx)?;
            parts.push(saule_type(&elem, ctx, fn_sig)?.0);
        }
        return Ok(format!("({})", parts.join(", ")));
    }
    let concrete = resolve_self(ty, ctx)?;
    Ok(saule_type(&concrete, ctx, fn_sig)?.0)
}

/// Map a Rust type to its Saule type string. Returns `(saule_type,
/// is_optional)`, where `is_optional` is true for `Option<T>`.
pub(crate) fn saule_type(
    ty: &Type,
    ctx: &mut TypeCtx<'_>,
    fn_sig: Option<&str>,
) -> syn::Result<(String, bool)> {
    if let Type::Tuple(t) = ty {
        if t.elems.is_empty() {
            return Ok(("nil".to_string(), false));
        }
        return Err(syn::Error::new_spanned(
            ty,
            "a tuple is only allowed as a return type (a multi-value return)",
        ));
    }
    if let Type::Reference(r) = ty {
        if is_str_like(&r.elem) {
            return Ok(("string".to_string(), false));
        }
        return Err(syn::Error::new_spanned(
            ty,
            "a reference can only be a parameter; return an owned value or an `SObject<T>`",
        ));
    }
    if let Type::Paren(p) = ty {
        return saule_type(&p.elem, ctx, fn_sig);
    }

    let Type::Path(tp) = ty else {
        return Err(unsupported(ty));
    };
    let Some(seg) = tp.path.segments.last() else {
        return Err(unsupported(ty));
    };

    let ident = seg.ident.to_string();
    let plain = |s: &str| Ok((s.to_string(), false));
    match ident.as_str() {
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => plain("integer"),
        "f32" | "f64" => plain("float"),
        "bool" => plain("boolean"),
        "String" | "str" => plain("string"),
        // The `S*` bridge types carry the same wire shape as their primitive,
        // with helper methods for the package author.
        "SInteger" => plain("integer"),
        "SFloat" => plain("float"),
        "SBool" => plain("boolean"),
        "SString" => plain("string"),
        // `STable<i64>` → `table<integer>`, `STable<T>` → `table<T>`,
        // bare `STable` → `table`. The element type is the table's own,
        // never a callback's, so `sig(...)` does not reach into it.
        "STable" => match first_generic(seg) {
            Some(inner) => {
                let (it, _) = saule_type(inner, ctx, None)?;
                Ok((format!("table<{it}>"), false))
            }
            None => plain("table"),
        },
        // Owned Rust collections cross as tables, copied at the boundary.
        "Vec" => {
            let inner = first_generic(seg).ok_or_else(|| unsupported(ty))?;
            let (it, _) = saule_type(inner, ctx, None)?;
            Ok((format!("table<{it}>"), false))
        }
        "HashMap" | "BTreeMap" => {
            let (k, v) = two_generics(seg).ok_or_else(|| unsupported(ty))?;
            let (kt, _) = saule_type(k, ctx, None)?;
            let (vt, _) = saule_type(v, ctx, None)?;
            Ok((format!("table<{kt}, {vt}>"), false))
        }
        "SFunction" => match fn_sig {
            Some(s) => {
                record_tvars(s, &mut ctx.tvars);
                plain(s)
            }
            None => Err(syn::Error::new_spanned(
                ty,
                "`SFunction` is a bare handle — declare the signature Saule should \
                 check calls against, e.g. `#[saule_export(class = \"Util\", \
                 name = \"map\", sig(f = \"fn(T) -> T\"))]`",
            )),
        },
        // A value typed as a generic type parameter: `SElem<T>` → `T`.
        "SElem" => {
            let inner = first_generic(seg).ok_or_else(|| unsupported(ty))?;
            saule_type(inner, ctx, None)
        }
        // A dynamically-typed value crossing the ABI.
        "SValue" => plain("any"),
        // Type-variable markers render as their own name and become the
        // signature's type parameters.
        "T" | "U" | "V" | "W" => {
            if !ctx.tvars.iter().any(|v| v == &ident) {
                ctx.tvars.push(ident.clone());
            }
            Ok((ident, false))
        }
        "Option" => {
            let inner = first_generic(seg).ok_or_else(|| unsupported(ty))?;
            let (inner_ty, _) = saule_type(inner, ctx, fn_sig)?;
            // `?` binds to the return type inside a function type, so
            // `Option<SFunction>` has to parenthesise: `(fn() -> nil)?`.
            if inner_ty.starts_with("fn(") || inner_ty.starts_with("fn<") {
                Ok((format!("({inner_ty})?"), true))
            } else {
                Ok((format!("{inner_ty}?"), true))
            }
        }
        // A shared handle to an object of a `#[saule_class]`.
        "SObject" => {
            let inner = first_generic(seg).ok_or_else(|| unsupported(ty))?;
            let name = named_class(inner).ok_or_else(|| unsupported(inner))?;
            plain(&name)
        }
        "Self" => ctx
            .self_name()
            .map(|n| (n, false))
            .ok_or_else(|| syn::Error::new_spanned(ty, "`Self` outside `#[saule_methods]`")),
        // Anything else names a type the package itself declares — a
        // `#[saule_class]` or a `#[saule_enum]`. Its Saule name is its Rust
        // name. If it is neither, the `FromSaule` / `IntoSaule` bound on the
        // generated code says so at the call site.
        _ if matches!(seg.arguments, PathArguments::None) => plain(&ident),
        _ => Err(unsupported(ty)),
    }
}

/// The Saule class name for a type that could be a `#[saule_class]`: a plain
/// path with no generic arguments, named by its last segment. `None` for
/// primitives and for the SDK's own bridge types, which are never classes.
pub(crate) fn named_class(ty: &Type) -> Option<String> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if !matches!(seg.arguments, PathArguments::None) {
        return None;
    }
    let name = seg.ident.to_string();
    let reserved = [
        "i8",
        "i16",
        "i32",
        "i64",
        "i128",
        "isize",
        "u8",
        "u16",
        "u32",
        "u64",
        "u128",
        "usize",
        "f32",
        "f64",
        "bool",
        "String",
        "str",
        "SInteger",
        "SFloat",
        "SBool",
        "SString",
        "STable",
        "SFunction",
        "SValue",
        "SElem",
        "SObject",
        "T",
        "U",
        "V",
        "W",
        "Self",
    ];
    (!reserved.contains(&name.as_str())).then_some(name)
}

/// Replace every `Self` in `ty` — `Self`, `&mut Self`, `SObject<Self>`,
/// `Option<Self>` — with the `impl`'s self type. The generated shims are
/// free functions outside the `impl`, where `Self` means nothing.
pub(crate) fn resolve_self(ty: &Type, ctx: &TypeCtx<'_>) -> syn::Result<Type> {
    use syn::visit_mut::VisitMut;

    struct Replace<'a> {
        with: Option<&'a Type>,
        failed: Option<syn::Error>,
    }
    impl VisitMut for Replace<'_> {
        fn visit_type_mut(&mut self, ty: &mut Type) {
            if let Type::Path(tp) = ty
                && tp.qself.is_none()
                && tp.path.is_ident("Self")
            {
                match self.with {
                    Some(t) => *ty = t.clone(),
                    None if self.failed.is_none() => {
                        self.failed = Some(syn::Error::new_spanned(
                            &*ty,
                            "`Self` outside `#[saule_methods]`",
                        ));
                    }
                    None => {}
                }
                return;
            }
            syn::visit_mut::visit_type_mut(self, ty);
        }
    }

    let mut out = ty.clone();
    let mut v = Replace {
        with: ctx.self_ty,
        failed: None,
    };
    v.visit_type_mut(&mut out);
    match v.failed {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

fn is_str_like(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.is_ident("str") || tp.path.is_ident("String"))
}

/// Record the generic markers (`T`/`U`/`V`/`W`) a hand-written `sig(...)`
/// string mentions, so the emitted signature carries the matching
/// `fn<T, …>` prefix even when no Rust parameter names them.
fn record_tvars(sig: &str, tvars: &mut Vec<String>) {
    for word in sig.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if matches!(word, "T" | "U" | "V" | "W") && !tvars.iter().any(|v| v == word) {
            tvars.push(word.to_string());
        }
    }
}

/// The first type argument of a path segment: `X` in `Option<X>`.
fn first_generic(seg: &syn::PathSegment) -> Option<&Type> {
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

fn two_generics(seg: &syn::PathSegment) -> Option<(&Type, &Type)> {
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let mut types = args.args.iter().filter_map(|a| match a {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    });
    Some((types.next()?, types.next()?))
}

/// `X` when `ty` is `<wrapper><X>`, e.g. `generic_arg(ty, "Option")`.
pub(crate) fn generic_arg<'t>(ty: &'t Type, wrapper: &str) -> Option<&'t Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != wrapper {
        return None;
    }
    first_generic(seg)
}

fn unsupported(ty: &Type) -> syn::Error {
    syn::Error::new_spanned(
        ty,
        "unsupported type for a Saule export; expected an integer or float type, \
         bool, String / &str, Option<_>, Vec<_>, HashMap<_, _>, one of the `S*` \
         bridge types, or a `#[saule_class]` / `#[saule_enum]` type",
    )
}
