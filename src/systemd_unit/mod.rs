mod constants;
mod parser;
mod path_buf_ext;
mod quoted;
mod split;
mod unit;
mod unit_file;
mod value;

pub use self::constants::*;
pub use self::path_buf_ext::*;
pub use self::quoted::{quote_value, quote_words, unquote_value};
pub use self::split::{SplitStrv, SplitWord};
pub use self::unit::*;
pub use self::unit_file::*;
pub(crate) use self::value::*;

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
