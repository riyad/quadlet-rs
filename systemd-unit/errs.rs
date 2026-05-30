use std::io;

use crate::parser;

// TODO: mimic https://doc.rust-lang.org/std/num/enum.IntErrorKind.html
#[derive(Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("value must be one of `1`, `yes`, `true`, `on`, `0`, `no`, `false`, `off`")]
    ParseBool,
    #[error("failed unquoting value: {0}")]
    Unquoting(String),
    #[error("failed to parse unit file: {0}")]
    Unit(#[from] parser::ParseError),
}

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Unit(#[from] Error),
}
