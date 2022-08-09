use once_cell::sync::Lazy;
use ordered_multimap::ListOrderedMultimap;
use super::Error;
use super::unquote_value;
use super::parse_bool;

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
pub(crate) struct EntryValue {
    pub(crate) raw: EntryRawValue,
    pub(crate) unquoted: String,
}

impl EntryValue {
    pub fn raw(&self) -> &String {
        &self.raw
    }

    pub fn unquoted(&self) -> &String {
        &self.unquoted
    }

    pub fn to_bool(&self) -> Result<bool, super::Error> {
        parse_bool(self.raw.as_str())
    }
}

impl From<&str> for EntryValue {
    fn from(s: &str) -> Self {
        Self {
            raw: s.to_owned(),
        }
    }
}

impl From<String> for EntryValue {
    fn from(s: String) -> Self {
        Self {
            raw: s,
        }
    }
}

pub(crate) type SectionKey = String;
