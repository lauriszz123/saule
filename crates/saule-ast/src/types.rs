//! Type ascriptions.

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// `integer`, `string`, `Player`, ...
    Named(String),
    /// `T?`
    Nullable(Box<Type>),
    /// `table<T>` (array-style, key implicit `integer`) when `key` is `None`;
    /// `table<K, V>` (hashmap-style) when `key` is `Some(K)`.
    Table {
        key: Option<Box<Type>>,
        value: Box<Type>,
    },
    /// `(A, B, C)` — currently used primarily for multi-return signatures.
    Tuple(Vec<Type>),
    /// `fn(A, B) -> R`
    Function { params: Vec<Type>, ret: Box<Type> },
    /// `Result<integer>`, `Box<string>`, `Pair<A, B>` — a user-declared
    /// generic applied to its arguments.
    ///
    /// `args` is never empty; a name with no arguments is [`Type::Named`], so
    /// there is exactly one spelling for `Player` and code matching on
    /// `Named` keeps working unchanged.
    ///
    /// `table<K, V>` stays its own [`Type::Table`] variant rather than
    /// becoming a `Generic`. Tables are the one built-in container, the
    /// checker special-cases them in a dozen places (element invariance,
    /// literal checking, the array/map split), and folding them in here would
    /// have meant rewriting all of that to gain nothing.
    ///
    /// The payload is behind one `Box` so this variant costs a pointer.
    /// Inline, `{ name: String, args: Vec<Type> }` is 48 bytes and becomes
    /// the largest variant, widening `Type` from 32 to 48 — and `Type` is
    /// embedded in `Expr`, so every expression node in every program pays
    /// for it whether or not it mentions a generic. `tests/node_size.rs` is
    /// the guard that caught exactly that.
    Generic(Box<GenericType>),
}

/// The payload of [`Type::Generic`] — a generic's name and the arguments
/// applied to it.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericType {
    pub name: String,
    pub args: Vec<Type>,
}

impl Type {
    /// `Name<args…>`. The arguments must be non-empty; a bare name is
    /// [`Type::Named`], so there is exactly one spelling for `Player`.
    pub fn generic(name: impl Into<String>, args: Vec<Type>) -> Type {
        debug_assert!(
            !args.is_empty(),
            "a generic application with no arguments is `Type::Named`"
        );
        Type::Generic(Box::new(GenericType {
            name: name.into(),
            args,
        }))
    }
}
