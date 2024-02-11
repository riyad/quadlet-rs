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
pub use self::quoted::*;
pub use self::split::*;
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
