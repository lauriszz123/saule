//! Turning a resolved parameter list into a [`SignatureHelp`].
//!
//! The label reads `name(p1: T1, p2: T2)`, with the active parameter
//! highlighted by byte ranges into that label rather than by index, which
//! is what lets clients underline the right span.

use saule_ast::{Param, Type};
use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, SignatureHelp, SignatureInformation,
};

use super::*;

/// Drop signatures that take no arguments, and the whole response if
/// nothing is left.
///
/// A parameterless call has nothing to tell you, and IntelliJ renders it
/// as a literal `<no parameters>` row — noise sitting next to the call
/// you're actually filling in, as with `update(dt: Timer.getDelta(`.
///
/// The AST path filters earlier, in [`help_from_module`], where spans
/// are still around to reselect the enclosing level. This is the same
/// rule for the textual fallback, which only ever has one signature.
pub(crate) fn drop_parameterless(help: SignatureHelp) -> Option<SignatureHelp> {
    let was_active = help
        .active_signature
        .and_then(|i| help.signatures.get(i as usize))
        .map(|s| s.label.clone());

    let signatures: Vec<SignatureInformation> = help
        .signatures
        .into_iter()
        .filter(|s| s.parameters.as_ref().is_some_and(|p| !p.is_empty()))
        .collect();
    if signatures.is_empty() {
        return None;
    }

    // If the caret was in the parameterless call we just removed, fall
    // back to the outermost surviving level rather than showing nothing.
    let active = was_active
        .and_then(|l| signatures.iter().position(|s| s.label == l))
        .unwrap_or(0);
    let active_parameter = signatures[active].active_parameter;
    Some(SignatureHelp {
        signatures,
        active_signature: Some(active as u32),
        active_parameter,
    })
}

/// Render a `NativeSig` as a `SignatureHelp`. Native sigs have no
/// param names, so we synthesize `arg0`, `arg1`, ... — cheap, but
/// good enough for stdlib calls where the type is the load-bearing
/// piece of information anyway.
pub(crate) fn build_help_native(
    display: &str,
    qname: &str,
    sig: &saule_typeck::sigs::NativeSig,
    args: &[CallArgInfo],
    offset: usize,
    multiline: bool,
) -> Option<SignatureHelp> {
    let names = crate::server::native_names::param_names(qname, sig);
    let mut label = String::new();
    label.push_str(display);
    label.push('(');
    let mut param_ranges: Vec<(u32, u32)> = Vec::new();
    let positional_n = sig.params.len();
    // Same separator rule as `render_signature_label`: the comma stays
    // on the line it ends, the next parameter starts the following one.
    let sep = |label: &mut String, first: bool| {
        if !first {
            label.push(',');
        }
        if multiline {
            label.push_str("\n  ");
        } else if !first {
            label.push(' ');
        }
    };
    for (i, ty) in sig.params.iter().enumerate() {
        sep(&mut label, i == 0);
        let start = utf16_len(&label);
        let pname = names.get(i).map(|s| s.as_str()).unwrap_or("value");
        label.push_str(pname);
        label.push_str(": ");
        label.push_str(&render_type(ty));
        let end = utf16_len(&label);
        param_ranges.push((start, end));
    }
    if let Some(var_ty) = &sig.variadic {
        sep(&mut label, positional_n == 0);
        let start = utf16_len(&label);
        let vname = names
            .get(positional_n)
            .map(|s| s.as_str())
            .unwrap_or("rest");
        label.push_str("...");
        label.push_str(vname);
        label.push_str(": ");
        label.push_str(&render_type(var_ty));
        let end = utf16_len(&label);
        param_ranges.push((start, end));
    }
    if multiline && !param_ranges.is_empty() {
        label.push('\n');
    }
    label.push(')');
    if !sig.returns.is_empty() {
        label.push_str(" -> ");
        let parts: Vec<String> = sig.returns.iter().map(render_type).collect();
        label.push_str(&parts.join(", "));
    }

    let arity_with_var = param_ranges.len();
    let active = active_parameter(args, offset, arity_with_var);

    let parameters = param_ranges
        .into_iter()
        .map(|(s, e)| ParameterInformation {
            label: ParameterLabel::LabelOffsets([s, e]),
            documentation: None,
        })
        .collect::<Vec<_>>();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active as u32),
    })
}

/// Build help for a user-defined free top-level function. We pull
/// `Param` records (with names!) directly from the AST, so the
/// rendering matches what `build_help` produces for class methods.
pub(crate) fn build_help_user_fn(
    name: &str,
    params: &[Param],
    return_ty: &Option<Type>,
    args: &[CallArgInfo],
    offset: usize,
    multiline: bool,
) -> Option<SignatureHelp> {
    use saule_semantic::MethodSig;
    let sig = MethodSig {
        is_static: false,
        is_private: false,
        type_params: Vec::new(),
        params: params.to_vec(),
        return_ty: return_ty.clone(),
    };
    build_help(name, Some(sig), args, offset, multiline)
}

/// Render `name(p1: T1, p2: T2, ...) -> R`, or the same across lines
/// when `multiline`, and record each parameter's `[start, end)` UTF-16
/// offsets within the label so the editor can highlight the active one.
///
/// The multi-line form mirrors the call the reader is looking at: a
/// twelve-parameter widget written one argument per line gets a
/// signature shaped the same way, instead of a single line that runs
/// off the side of the popup.
pub(crate) fn render_signature_label(
    name: &str,
    params: &[Param],
    return_ty: Option<&Type>,
    multiline: bool,
) -> (String, Vec<(u32, u32)>) {
    let mut label = String::from(name);
    label.push('(');
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            label.push(',');
        }
        if multiline {
            label.push_str("\n  ");
        } else if i > 0 {
            label.push(' ');
        }
        let start = utf16_len(&label);
        label.push_str(&p.name);
        label.push_str(": ");
        label.push_str(&render_type(&p.ty));
        if p.variadic {
            label.push_str("...");
        }
        if p.default.is_some() {
            label.push_str(" = …");
        }
        ranges.push((start, utf16_len(&label)));
    }
    if multiline && !params.is_empty() {
        label.push('\n');
    }
    label.push(')');
    if let Some(rt) = return_ty {
        label.push_str(" -> ");
        label.push_str(&render_type(rt));
    }
    (label, ranges)
}

pub(crate) fn build_help(
    name: &str,
    sig: Option<saule_semantic::MethodSig>,
    args: &[CallArgInfo],
    offset: usize,
    multiline: bool,
) -> Option<SignatureHelp> {
    let sig = sig?;
    let (label, param_ranges) =
        render_signature_label(name, &sig.params, sig.return_ty.as_ref(), multiline);

    let parameters = param_ranges
        .into_iter()
        .map(|(s, e)| ParameterInformation {
            label: ParameterLabel::LabelOffsets([s, e]),
            documentation: None,
        })
        .collect::<Vec<_>>();

    // Now that the signature is known, point each `name: value`
    // argument at the slot its key names.
    let resolved: Vec<CallArgInfo> = args
        .iter()
        .map(|a| CallArgInfo {
            named_index: a
                .name
                .as_ref()
                .and_then(|n| sig.params.iter().position(|p| &p.name == n)),
            ..a.clone()
        })
        .collect();
    let active = active_parameter(&resolved, offset, sig.params.len());

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active as u32),
    })
}

/// Index of the currently-edited parameter slot. Counts how many
/// completed positional/named arguments precede the cursor — a
/// trailing `,` means the *next* param is active. Clamped to the
/// signature's arity so we never report an out-of-range slot.
pub(crate) fn active_parameter(args: &[CallArgInfo], offset: usize, arity: usize) -> usize {
    let mut idx = 0;
    for arg in args {
        // The cursor is inside this arg's span -> this slot is active.
        if offset >= arg.span.start && offset <= arg.span.end {
            return arg.named_index.unwrap_or(idx).min(arity.saturating_sub(1));
        }
        // Cursor lies past the arg -> we've completed it; advance.
        if offset > arg.span.end {
            idx += 1;
        }
    }
    idx.min(arity.saturating_sub(1))
}

/// Length of `s` in UTF-16 code units.
///
/// `ParameterInformation.label` offsets are indices into the signature
/// label *as the client sees it*, and LSP defines string positions in
/// UTF-16 code units — not Rust byte offsets. The label can contain
/// non-ASCII (`" = …"` for a defaulted parameter), so handing out
/// `label.len()` makes every offset past that point too large and the
/// client slices out of bounds.
pub(crate) fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

pub(crate) fn render_type(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", render_type(inner)),
        Type::Function { params, ret } => {
            let ps = params
                .iter()
                .map(render_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn({ps}) -> {}", render_type(ret))
        }
        Type::Table { key, value } => match key {
            Some(k) => format!("table<{}, {}>", render_type(k), render_type(value)),
            None => format!("table<{}>", render_type(value)),
        },
        Type::Tuple(parts) => format!(
            "({})",
            parts.iter().map(render_type).collect::<Vec<_>>().join(", ")
        ),
    }
}

// ──────────────────────────────────────────────────────────────────────
// AST walk: locate the smallest enclosing call
// ──────────────────────────────────────────────────────────────────────
