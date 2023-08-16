mod constants;
mod parser;
mod quoted;
mod split;
mod unit;
mod unit_file;
mod value;

use std::fmt;
use std::io;

pub use self::constants::*;
pub use self::quoted::*;
pub use self::split::*;
pub use self::unit::*;
pub use self::unit_file::*;
pub(crate) use self::value::*;

// TODO: mimic https://doc.rust-lang.org/std/num/enum.IntErrorKind.html
// TODO: use thiserror?
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Error {
    ParseBool,
    Unquoting(String),
    Unit(parser::ParseError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ParseBool => {
                write!(
                    f,
                    "value must be one of `1`, `yes`, `true`, `on`, `0`, `no`, `false`, `off`"
                )
            }
            Error::Unquoting(msg) => {
                write!(f, "failed unquoting value: {msg}")
            }
            Error::Unit(e) => {
                write!(f, "failed to parse unit file: {e}")
            }
        }
    }
}

impl From<parser::ParseError> for Error {
    fn from(e: parser::ParseError) -> Self {
        Error::Unit(e)
    }
}

pub(crate) fn parse_bool(s: &str) -> Result<bool, Error> {
    if ["1", "yes", "true", "on"].contains(&s) {
        return Ok(true);
    } else if ["0", "no", "false", "off"].contains(&s) {
        return Ok(false);
    }

    Err(Error::ParseBool)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_bool {
        use super::*;

        #[test]
        fn fails_with_empty_input() {
            assert_eq!(parse_bool("").err(), Some(Error::ParseBool));
        }

        #[test]
        fn true_with_truthy_input() {
            assert_eq!(parse_bool("1").ok(), Some(true));
            assert_eq!(parse_bool("on").ok(), Some(true));
            assert_eq!(parse_bool("yes").ok(), Some(true));
            assert_eq!(parse_bool("true").ok(), Some(true));
        }

        #[test]
        fn false_with_falsy_input() {
            assert_eq!(parse_bool("0").ok(), Some(false));
            assert_eq!(parse_bool("off").ok(), Some(false));
            assert_eq!(parse_bool("no").ok(), Some(false));
            assert_eq!(parse_bool("false").ok(), Some(false));
        }

        #[test]
        fn fails_with_non_boolean_input() {
            assert_eq!(parse_bool("foo").err(), Some(Error::ParseBool));
        }
    }
}
