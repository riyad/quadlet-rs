use super::{parse_bool, quote_value, unquote_value, Error};
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
        EMPTY.get_or_init(|| Entries::default())
    }
}

pub(crate) type EntryKey = String;

pub(crate) type EntryRawValue = String;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct EntryValue {
    raw: EntryRawValue,
    unquoted: String,
}

impl EntryValue {
    pub fn from_unquoted<S: Into<String>>(unquoted: S) -> Self {
        let unquoted = unquoted.into();
        Self {
            raw: quote_value(unquoted.as_str()),
            unquoted,
        }
    }

    pub fn raw(&self) -> &String {
        &self.raw
    }

    pub fn unquote(&self) -> String {
        self.try_unquote().expect("parsing error")
    }

    #[deprecated = "use unquote() or try_unquote()"]
    pub fn unquoted(&self) -> &String {
        &self.unquoted
    }

    pub fn to_bool(&self) -> Result<bool, Error> {
        let trimmed = self.raw.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }

        parse_bool(trimmed)
    }

    pub fn try_from_raw<S: Into<String>>(raw: S) -> Result<Self, Error> {
        let raw = raw.into();
        Ok(Self {
            unquoted: unquote_value(raw.as_str())?,
            raw,
        })
    }

    pub fn try_unquote(&self) -> Result<String, Error> {
        unquote_value(self.raw.as_str())
    }
}

/// experimental: not sure if this is the right way
impl From<&str> for EntryValue {
    fn from(unquoted: &str) -> Self {
        Self::from_unquoted(unquoted)
    }
}

/// experimental: not sure if this is the right way
impl From<String> for EntryValue {
    fn from(unquoted: String) -> Self {
        Self::from_unquoted(unquoted)
    }
}

/// experimental: not sure if this is the right way
impl FromStr for EntryValue {
    type Err = Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::try_from_raw(raw)
    }
}

pub(crate) type SectionKey = String;

#[cfg(test)]
mod tests {
    use super::*;

    mod entry_value {
        use super::*;

        mod from_unquoted {
            use super::*;

            #[test]
            fn value_gets_quoted() {
                let input = "foo=\"bar\"";
                let value = EntryValue::from_unquoted(input);

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
                let value = EntryValue {
                    raw: "foo \"bar\" foo=\"bar\"".into(),
                    unquoted: String::new(),
                };

                assert_eq!(value.try_unquote(), Ok("foo bar foo=\"bar\"".into()),);
            }

            #[test]
            fn error_for_invalid_value() {
                let value = EntryValue {
                    raw: "\\x00".into(),
                    unquoted: String::new(),
                };

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
                let value = EntryValue {
                    raw: "\\x00".into(),
                    unquoted: String::new(),
                };

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
            let value = EntryValue::try_from_raw(input).unwrap();

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
}
