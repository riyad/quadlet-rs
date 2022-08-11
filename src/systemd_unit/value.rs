use once_cell::sync::Lazy;
use ordered_multimap::ListOrderedMultimap;
use super::{Error, parse_bool, quote_value, unquote_value};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Entries {
    pub(crate) data: ListOrderedMultimap<EntryKey, EntryValue>,
}

impl Default for &Entries {
    fn default() -> Self {
        static EMPTY: Lazy<Entries> = Lazy::new(|| Entries::default());
        &EMPTY
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

    pub fn unquoted(&self) -> &String {
        &self.unquoted
    }

    pub fn to_bool(&self) -> Result<bool, super::Error> {
        parse_bool(self.raw.as_str())
    }

    pub fn try_from_raw<S: Into<String>>(raw: S) -> Result<Self, Error> {
        let raw = raw.into();
        Ok(Self {
            unquoted: unquote_value(raw.as_str())?,
            raw: raw,
        })
    }
}

impl From<&str> for EntryValue {
    fn from(s: &str) -> Self {
        Self::from_unquoted(s)
    }
}

impl From<String> for EntryValue {
    fn from(s: String) -> Self {
        Self::from_unquoted(s)
    }
}

pub(crate) type SectionKey = String;

mod tests {
    mod entry_value {
        mod from_unquoted {
            use super::super::super::EntryValue;

            #[test]
            fn value_gets_quoted() {
                let input = "foo=\"bar\"";
                let value = EntryValue::from_unquoted(input);

                assert_eq!(
                    value.unquoted(),
                    input
                );
                assert_eq!(
                    value.raw(),
                    "foo=\\\"bar\\\""
                );
            }
        }

        mod try_from_raw {
            use super::super::super::EntryValue;

            #[test]
            fn value_gets_unquoted() {
                let input = "foo \"bar\"";
                let value = EntryValue::try_from_raw(input).unwrap();

                assert_eq!(
                    value.raw(),
                    input
                );
                assert_eq!(
                    value.unquoted(),
                    "foo bar"
                );
            }
        }
    }

    mod from_str_for_entry_value {
        use super::super::{EntryValue, quote_value};

        #[test]
        fn value_gets_quoted() {
            let input = "foo=\"bar\"";
            let value: EntryValue = input.into();

            assert_eq!(
                value.unquoted(),
                input
            );
            assert_eq!(
                value.raw(),
                "foo=\\\"bar\\\""
            );
        }
    }
    mod from_string_for_entry_value {
        use super::super::{EntryValue, quote_value};

        #[test]
        fn value_gets_quoted() {
            let input = "foo=\"bar\"".to_string();
            let value: EntryValue = input.clone().into();

            assert_eq!(
                value.unquoted(),
                &input
            );
            assert_eq!(
                value.raw(),
                "foo=\\\"bar\\\""
            );
        }
    }
}