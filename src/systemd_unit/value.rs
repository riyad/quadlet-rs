use super::quoted::{escape_value, unescape_value};
use super::split::{SplitStrv, SplitWord};
use super::Error;
use core::fmt;
use ordered_multimap::ListOrderedMultimap;
use std::str::FromStr;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Entries {
    pub(crate) data: ListOrderedMultimap<EntryKey, EntryValue>,
}

impl Default for &Entries {
    fn default() -> Self {
        static EMPTY: OnceLock<Entries> = OnceLock::new();
        EMPTY.get_or_init(Entries::default)
    }
}

pub(crate) type EntryKey = String;

pub(crate) type EntryRawValue = String;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct EntryValue(EntryRawValue);

impl EntryValue {
    pub fn from_raw<S: Into<String>>(raw: S) -> Self {
        Self::try_from_raw(raw).expect("value not correctly quoted")
    }

    pub fn new(unquoted: &str) -> Self {
        Self(escape_value(unquoted))
    }

    pub(crate) fn raw(&self) -> &String {
        &self.0
    }

    pub fn split_strv(&self) -> SplitStrv<'_> {
        SplitStrv::new(self.0.as_str())
    }

    pub fn split_words(&self) -> SplitWord<'_> {
        SplitWord::new(self.0.as_str())
    }

    pub fn to_bool(&self) -> Result<bool, Error> {
        let trimmed = self.0.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }

        parse_bool(trimmed)
    }

    pub fn try_from_raw<S: Into<String>>(raw: S) -> Result<Self, Error> {
        let raw = raw.into();
        let _ = unescape_value(raw.as_str())?;
        Ok(Self(raw))
    }

    pub fn try_unquote(&self) -> Result<String, Error> {
        unescape_value(self.0.as_str())
    }

    // pub fn to_string(&self) -> String {
    //     self.unquote()
    // }

    pub fn unquote(&self) -> String {
        self.try_unquote().expect("parsing error")
    }
}

/// experimental: not sure if this is the right way
impl From<&str> for EntryValue {
    fn from(unquoted: &str) -> Self {
        Self::new(unquoted)
    }
}

/// experimental: not sure if this is the right way
impl From<String> for EntryValue {
    fn from(unquoted: String) -> Self {
        Self::new(unquoted.as_str())
    }
}

/// experimental: not sure if this is the right way
impl FromStr for EntryValue {
    type Err = Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_raw(raw))
    }
}

impl fmt::Display for EntryValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.unquote())
    }
}

pub(crate) type SectionKey = String;

fn parse_bool(s: &str) -> Result<bool, Error> {
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

    mod entry_value {
        use super::*;

        mod new {
            use super::*;

            #[test]
            fn value_gets_quoted() {
                let input = "foo=\"bar\"";
                let value = EntryValue::new(input);

                assert_eq!(value.unquote(), input);
                assert_eq!(value.raw(), "foo=\\\"bar\\\"");
            }
        }

        mod to_bool {
            use super::*;

            #[test]
            fn known_true_values_are_true() {
                for input in ["1", "yes", "true", "on"] {
                    let value = EntryValue::from_str(input).unwrap();

                    assert_eq!(value.to_bool(), Ok(true),)
                }
            }

            #[test]
            fn known_false_values_are_false() {
                for input in ["0", "no", "false", "off"] {
                    let value = EntryValue::from_str(input).unwrap();

                    assert_eq!(value.to_bool(), Ok(false),)
                }
            }

            #[test]
            fn error_for_empty_value() {
                let input = "";

                let value = EntryValue::from_str(input).unwrap();

                assert_eq!(value.to_bool(), Ok(false),)
            }

            #[test]
            fn error_for_whitespace_value() {
                let input = " ";

                let value = EntryValue::from_str(input).unwrap();

                assert_eq!(value.to_bool(), Ok(false),)
            }

            #[test]
            fn error_for_non_bool_value() {
                let input = "foo";

                let value = EntryValue::from_str(input).unwrap();

                assert_eq!(value.to_bool(), Err(Error::ParseBool),)
            }
        }

        mod try_from_raw {
            use super::*;

            #[test]
            fn value_gets_unquoted() {
                let input = "foo \"bar\"";
                let value = EntryValue::try_from_raw(input).unwrap();

                assert_eq!(value.raw(), input);
                assert_eq!(value.unquote(), "foo bar");
            }
        }

        mod try_unquote {
            use super::*;

            #[test]
            fn unquotes_value() {
                let value = EntryValue::from_raw("foo \"bar\" foo=\"bar\"");

                assert_eq!(value.try_unquote(), Ok("foo bar foo=\"bar\"".into()),);
            }

            #[test]
            fn error_for_invalid_value() {
                let value = EntryValue("\\x00".into());

                assert_eq!(
                    value.try_unquote(),
                    Err(Error::Unquoting(
                        "\\0 character not allowed in escape sequence".into()
                    )),
                );
            }
        }

        mod unquote {
            use super::*;

            #[test]
            #[should_panic]
            fn panics_on_parse_error() {
                let value = EntryValue::from_raw("\\x00");

                value.unquote();
            }
        }
    }

    mod from_ref_str_for_entry_value {
        use super::*;

        #[test]
        fn value_gets_quoted() {
            let input = "foo=\"bar\"";
            let value: EntryValue = input.into();

            assert_eq!(value.unquote(), input);
            assert_eq!(value.raw(), "foo=\\\"bar\\\"");
        }
    }

    mod from_str_for_entry_value {
        use super::*;

        #[test]
        fn value_gets_unquoted() {
            let input = "foo \"bar\"";
            let value = EntryValue::from_raw(input);

            assert_eq!(value.raw(), input);
            assert_eq!(value.unquote(), "foo bar");
        }
    }

    mod from_string_for_entry_value {
        use super::*;

        #[test]
        fn value_gets_quoted() {
            let input = "foo=\"bar\"".to_string();
            let value: EntryValue = input.clone().into();

            assert_eq!(value.unquote(), input);
            assert_eq!(value.raw(), "foo=\\\"bar\\\"");
        }
    }

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
