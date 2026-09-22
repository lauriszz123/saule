//! File handles backing `Value::File`.

use std::fmt;

pub enum FileHandle {
    Stdin(std::io::BufReader<std::io::Stdin>),
    Stdout,
    Stderr,
    Open {
        path: String,
        reader: Option<std::io::BufReader<std::fs::File>>,
        writer: Option<std::fs::File>,
    },
    Closed,
}

impl fmt::Debug for FileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileHandle::Stdin(_) => write!(f, "<file stdin>"),
            FileHandle::Stdout => write!(f, "<file stdout>"),
            FileHandle::Stderr => write!(f, "<file stderr>"),
            FileHandle::Open { path, .. } => write!(f, "<file {path}>"),
            FileHandle::Closed => write!(f, "<closed file>"),
        }
    }
}
