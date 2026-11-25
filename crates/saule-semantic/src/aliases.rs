//! Type aliases: `type Name = T`.
//!
//! An alias is another name for a type and nothing more. It declares no new
//! type and leaves nothing behind at run time, so the whole feature is one
//! rewrite: before a module is analysed, every type in it that names an
//! alias is replaced by what the alias stands for. The passes after that —
//! the rest of this crate, the typechecker, the compiler — never meet one.
//!
//! Two steps, in this order:
//!
//! * [`alias_scope`] works out what each alias in scope stands for, written
//!   with no alias left in it. That is where an alias defined in terms of
//!   itself is caught, since writing such a one out would never end.
//! * [`expand_aliases`] rewrites a module's types against that scope.
//!
//! [`crate::analyze`] and its siblings run both on a copy of the tree, so
//! the tree a caller holds keeps the names as written — which is what a
//! formatter prints and what an editor shows. The typechecker rewrites the
//! tree it is handed from the scope installed here (see
//! [`expand_installed`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use saule_ast::{Decl, Module, Stmt, Type, TypeSite};

use crate::error::SemanticError;
use crate::registry::substitute_type_params;
use crate::to_source_span;

/// What one alias stands for.
///
/// In a scope built by [`alias_scope`], `ty` mentions no alias: only real
/// types and the alias's own `type_params`.
#[derive(Clone, Debug, PartialEq)]
pub struct AliasInfo {
    /// The parameters declared with `<A, B>`, replaced at each use by the
    /// arguments written there.
    pub type_params: Vec<String>,
    pub ty: Type,
}

/// The aliases in scope, by the name each is known under locally.
pub type AliasRegistry = HashMap<String, AliasInfo>;

thread_local! {
    static ALIASES: RefCell<AliasRegistry> = RefCell::new(HashMap::new());
}

/// Read the aliases of the module last analysed.
pub fn with_aliases<R>(f: impl FnOnce(&AliasRegistry) -> R) -> R {
    ALIASES.with(|c| f(&c.borrow()))
}

/// What `name` stands for, when it is an alias in the module last analysed.
pub fn lookup_alias(name: &str) -> Option<AliasInfo> {
    with_aliases(|r| r.get(name).cloned())
}

pub(crate) fn install_aliases(aliases: AliasRegistry) {
    ALIASES.with(|c| *c.borrow_mut() = aliases);
}

pub(crate) fn clear_aliases() {
    ALIASES.with(|c| c.borrow_mut().clear());
}

/// The names that are types with no declaration, and the two words read as
/// types that are not. An alias may not take one: `type integer = float`
/// would change what every annotation in the file means.
const RESERVED: &[&str] = &[
    "integer", "float", "byte", "string", "bytes", "boolean", "table", "userdata", "thread", "any",
    "nil", "number", "function", "self",
];

/// One `type` declaration at the top of a module, as written.
struct Declared<'a> {
    name: &'a str,
    name_span: &'a Range<usize>,
    type_params: &'a [String],
    ty: &'a Type,
    exported: bool,
}

fn declared(module: &Module) -> Vec<Declared<'_>> {
    module
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => match &d.value {
                Decl::TypeAlias {
                    exported,
                    name,
                    name_span,
                    type_params,
                    ty,
                    ..
                } => Some(Declared {
                    name,
                    name_span,
                    type_params,
                    ty,
                    exported: *exported,
                }),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The classes, interfaces and enums `module` declares itself. One of them
/// keeps its name over an imported alias, as it does over an imported class.
fn declared_type_names(module: &Module) -> HashSet<&str> {
    module
        .stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => match &d.value {
                Decl::Class { name, .. }
                | Decl::Interface { name, .. }
                | Decl::Enum { name, .. } => Some(name.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The aliases `module` exports with `export type`, out of a scope
/// [`alias_scope`] built for it.
pub fn exported_aliases(module: &Module, scope: &AliasRegistry) -> AliasRegistry {
    declared(module)
        .into_iter()
        .filter(|d| d.exported)
        .filter_map(|d| Some((d.name.to_string(), scope.get(d.name)?.clone())))
        .collect()
}

/// Every alias in scope in `module`, each written out in full: the ones its
/// imports bring in (`imported`, already in that form) and the ones it
/// declares.
///
/// The errors are about the declarations themselves — a name taken twice, a
/// name that is already a type, an alias that contains itself. A mistake in
/// what an alias is *used* with is [`expand_aliases`]' to report.
///
/// A declaration in error is still in the scope it returns, standing for
/// `any`, so that one mistake is reported once and not again at every use.
pub fn alias_scope(
    module: &Module,
    imported: &AliasRegistry,
) -> (AliasRegistry, Vec<SemanticError>) {
    let own = declared(module);
    if own.is_empty() && imported.is_empty() {
        return (AliasRegistry::new(), Vec::new());
    }

    let mut errors = Vec::new();
    let types = declared_type_names(module);
    let mut scope: AliasRegistry = imported
        .iter()
        .filter(|(name, _)| !types.contains(name.as_str()))
        .map(|(name, info)| (name.clone(), info.clone()))
        .collect();

    // The declarations that stand, by name: the first of each.
    let mut pending: HashMap<&str, &Declared<'_>> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for d in &own {
        let problem = if RESERVED.contains(&d.name) {
            Some(SemanticError::AliasOfBuiltin {
                name: d.name.to_string(),
                span: to_source_span(d.name_span.clone()),
            })
        } else if pending.contains_key(d.name) || types.contains(d.name) {
            Some(SemanticError::DuplicateType {
                name: d.name.to_string(),
                span: to_source_span(d.name_span.clone()),
            })
        } else {
            None
        };
        match problem {
            Some(e) => errors.push(e),
            None => {
                pending.insert(d.name, d);
                order.push(d.name);
                // An alias declared here takes its name from an imported one.
                scope.remove(d.name);
            }
        }
    }

    let mut resolver = Resolver {
        pending,
        scope,
        stack: Vec::new(),
        errors,
    };
    for name in order {
        resolver.resolve(name);
    }
    (resolver.scope, resolver.errors)
}

/// Writes out the aliases a module declares, each after the ones it mentions.
struct Resolver<'a> {
    /// Declared, and not yet written out.
    pending: HashMap<&'a str, &'a Declared<'a>>,
    /// Written out: the imported ones from the start, then each declared one
    /// as it is finished.
    scope: AliasRegistry,
    /// The aliases being written out, outermost first. Meeting one of them
    /// again is a cycle.
    stack: Vec<&'a str>,
    errors: Vec<SemanticError>,
}

impl<'a> Resolver<'a> {
    fn resolve(&mut self, name: &'a str) {
        let Some(decl) = self.pending.get(name).copied() else {
            return;
        };
        if let Some(at) = self.stack.iter().position(|n| *n == name) {
            self.cycle(at);
            return;
        }

        self.stack.push(name);
        let mut mentioned = Vec::new();
        mentions(decl.ty, decl.type_params, &mut mentioned);
        for other in mentioned {
            if let Some((other, _)) = self.pending.get_key_value(other) {
                self.resolve(other);
            }
        }
        self.stack.pop();

        // A cycle through this one has already settled it.
        if self.pending.remove(name).is_none() {
            return;
        }
        let mut ty = decl.ty.clone();
        // A wrong number of arguments in here is reported when the module
        // is rewritten, which reaches this type where it is declared.
        expand_type(&mut ty, &self.scope, decl.type_params, &mut |_| {});
        self.scope.insert(
            name.to_string(),
            AliasInfo {
                type_params: decl.type_params.to_vec(),
                ty,
            },
        );
    }

    /// `stack[at..]` leads back to its own first entry. Report it once, at
    /// that entry, and settle every alias on it as `any`.
    fn cycle(&mut self, at: usize) {
        let on_it: Vec<&'a str> = self.stack[at..].to_vec();
        let first = on_it[0];
        let Some(decl) = self.pending.get(first).copied() else {
            return;
        };
        let mut path: Vec<String> = on_it.iter().map(|n| format!("`{n}`")).collect();
        path.push(format!("`{first}`"));
        self.errors.push(SemanticError::AliasCycle {
            name: first.to_string(),
            path: path.join(" → "),
            span: to_source_span(decl.name_span.clone()),
        });
        for name in on_it {
            if let Some(d) = self.pending.remove(name) {
                self.scope.insert(
                    name.to_string(),
                    AliasInfo {
                        type_params: d.type_params.to_vec(),
                        ty: Type::Named("any".into()),
                    },
                );
            }
        }
    }
}

/// The names `ty` mentions, other than the type parameters in `shadowed`.
fn mentions<'a>(ty: &'a Type, shadowed: &[String], out: &mut Vec<&'a str>) {
    match ty {
        Type::Named(n) => {
            if !shadowed.contains(n) {
                out.push(n);
            }
        }
        Type::Nullable(inner) => mentions(inner, shadowed, out),
        Type::Table { key, value } => {
            if let Some(k) = key {
                mentions(k, shadowed, out);
            }
            mentions(value, shadowed, out);
        }
        Type::Tuple(items) => items.iter().for_each(|t| mentions(t, shadowed, out)),
        Type::Function { params, ret } => {
            params.iter().for_each(|t| mentions(t, shadowed, out));
            mentions(ret, shadowed, out);
        }
        Type::Generic(g) => {
            if !shadowed.contains(&g.name) {
                out.push(&g.name);
            }
            g.args.iter().for_each(|t| mentions(t, shadowed, out));
        }
    }
}

/// An alias used with the wrong number of type arguments.
pub struct WrongArity {
    pub name: String,
    pub expected: usize,
    pub found: usize,
}

/// Replace every alias `ty` names by what it stands for in `scope`.
///
/// A name in `shadowed` is a type parameter where `ty` stands, and is left
/// alone whatever alias shares its name: inside `class Map<Key, Value>`,
/// `Key` is the parameter.
///
/// An alias given the wrong number of arguments goes to `report` and becomes
/// `any`, so the mistake is not reported a second time as a mismatch.
pub fn expand_type(
    ty: &mut Type,
    scope: &AliasRegistry,
    shadowed: &[String],
    report: &mut dyn FnMut(WrongArity),
) {
    match ty {
        Type::Named(name) => {
            if shadowed.contains(name) {
                return;
            }
            let Some(info) = scope.get(name.as_str()) else {
                return;
            };
            if info.type_params.is_empty() {
                *ty = info.ty.clone();
            } else {
                report(WrongArity {
                    name: name.clone(),
                    expected: info.type_params.len(),
                    found: 0,
                });
                *ty = Type::Named("any".into());
            }
        }
        Type::Generic(g) => {
            for arg in &mut g.args {
                expand_type(arg, scope, shadowed, report);
            }
            if shadowed.contains(&g.name) {
                return;
            }
            let Some(info) = scope.get(g.name.as_str()) else {
                return;
            };
            if info.type_params.len() == g.args.len() {
                *ty = substitute_type_params(&info.ty, &info.type_params, &g.args);
                // `Opt<string?>` with `type Opt<T> = T?` put a nullable
                // inside a nullable.
                one_layer_of_nil(ty);
            } else {
                report(WrongArity {
                    name: g.name.clone(),
                    expected: info.type_params.len(),
                    found: g.args.len(),
                });
                *ty = Type::Named("any".into());
            }
        }
        Type::Nullable(inner) => {
            expand_type(inner, scope, shadowed, report);
            // `Name?` where `Name` already allows nil is that same type, not
            // a second layer of "or nil".
            one_layer_of_nil(ty);
        }
        Type::Table { key, value } => {
            if let Some(k) = key {
                expand_type(k, scope, shadowed, report);
            }
            expand_type(value, scope, shadowed, report);
        }
        Type::Tuple(items) => {
            for item in items {
                expand_type(item, scope, shadowed, report);
            }
        }
        Type::Function { params, ret } => {
            for p in params {
                expand_type(p, scope, shadowed, report);
            }
            expand_type(ret, scope, shadowed, report);
        }
    }
}

/// Collapse every `T??` in `ty` to `T?`. A value either may be nil or may
/// not; there is no second "or nil" to add, and the checker has no reading
/// for one.
fn one_layer_of_nil(ty: &mut Type) {
    match ty {
        Type::Named(_) => {}
        Type::Nullable(inner) => {
            one_layer_of_nil(inner);
            if let Type::Nullable(_) = inner.as_ref() {
                let inner = std::mem::replace(inner.as_mut(), Type::Named(String::new()));
                *ty = inner;
            }
        }
        Type::Table { key, value } => {
            if let Some(k) = key {
                one_layer_of_nil(k);
            }
            one_layer_of_nil(value);
        }
        Type::Tuple(items) => items.iter_mut().for_each(one_layer_of_nil),
        Type::Function { params, ret } => {
            params.iter_mut().for_each(one_layer_of_nil);
            one_layer_of_nil(ret);
        }
        Type::Generic(g) => g.args.iter_mut().for_each(one_layer_of_nil),
    }
}

/// Rewrite every type written in `module` against `scope`, reporting each
/// alias used with the wrong number of type arguments.
pub fn expand_aliases(module: &mut Module, scope: &AliasRegistry) -> Vec<SemanticError> {
    let mut errors = Vec::new();
    if scope.is_empty() {
        return errors;
    }
    saule_ast::visit_types_mut(module, |ty, site: TypeSite<'_>| {
        expand_type(ty, scope, site.type_params, &mut |wrong| {
            errors.push(SemanticError::AliasArity {
                name: wrong.name,
                expected: wrong.expected,
                found: wrong.found,
                span: to_source_span(site.span.clone()),
            });
        });
    });
    errors
}

/// [`expand_aliases`] against the scope of the module last analysed — what a
/// pass that runs after [`crate::analyze`] calls on its own tree, since
/// analysis rewrote a copy.
///
/// Reports nothing: analysis already did. It is for a tree as the parser
/// left it, once: what an alias was replaced by is not read again, and may
/// well name a type the alias's own module knows by a name this one uses
/// for something else.
pub fn expand_installed(module: &mut Module) {
    with_aliases(|scope| {
        expand_aliases(module, scope);
    });
}

/// `module` as the passes after analysis read it: [`expand_installed`], on
/// a copy. A module with no alias in scope, which is most of them, is
/// borrowed as it stands.
pub fn written_out(module: &Module) -> std::borrow::Cow<'_, Module> {
    if with_aliases(|scope| scope.is_empty()) {
        return std::borrow::Cow::Borrowed(module);
    }
    let mut copy = module.clone();
    expand_installed(&mut copy);
    std::borrow::Cow::Owned(copy)
}
