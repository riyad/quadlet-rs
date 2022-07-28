use ordered_multimap::ListOrderedMultimap;

use super::parse_bool;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Entries {
    pub(crate) data: ListOrderedMultimap<EntryKey, EntryValue>,
}


pub(crate) type EntryKey = String;

pub(crate) type EntryRawValue = String;

#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct EntryValue {
    pub(crate) raw: EntryRawValue,
}

impl EntryValue {
    pub fn from_raw<S: Into<String>>(raw: S) -> Self {
        Self::from(raw.into())
    }

    pub fn raw(&self) -> &String {
        &self.raw
    }

    pub fn unquoted(&self) -> &String {
        // TODO: implement
        self.raw()
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
