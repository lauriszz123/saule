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
    /// `fn(A, B): R`
    Function { params: Vec<Type>, ret: Box<Type> },
}
