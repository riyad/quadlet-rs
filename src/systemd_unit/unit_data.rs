use ordered_multimap::list_ordered_multimap::ListOrderedMultimap;
use std::collections::HashMap;
use std::io;

use super::{parser, Entries, EntryValue, SectionKey};

#[derive(Clone, Debug, PartialEq)]
pub struct SystemdUnitData {
    pub(crate) sections: ListOrderedMultimap<SectionKey, Entries>,
}

impl SystemdUnitData {
    /// Appends `key=value` to last instance of `section`
    pub(crate) fn add<S, K>(&mut self, section: S, key: K, value: &str)
    where
        S: Into<String>,
        K: Into<String>,
    {
        self.add_entry_value(section.into(), key.into(), EntryValue::new(value));
    }

    /// Appends `key=value` to last instance of `section`
    pub(crate) fn add_raw<S, K>(
        &mut self,
        section: S,
        key: K,
        raw_value: &str,
    ) -> Result<(), super::Error>
    where
        S: Into<String>,
        K: Into<String>,
    {
        self.add_entry_value(
            section.into(),
            key.into(),
            EntryValue::try_from_raw(raw_value)?,
        );

        Ok(())
    }

    fn add_entry_value(&mut self, section: String, key: String, value: EntryValue) {
        self.sections
            .entry(section)
            .or_insert_entry(Entries::default())
            .into_mut()
            .data
            .append(key, value);
    }

    pub(crate) fn has_key(&self, section: &str, key: &str) -> bool {
        self.sections
            .get(section)
            .map_or(false, |e| e.data.contains_key(key))
    }

    /// Retrun `true` if there's an (non-empty) instance of section `name`
    pub(crate) fn has_section(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }

    /// Number of unique sections (i.e. with different names)
    pub fn len(&self) -> usize {
        self.sections.keys_len()
    }

    /// Load from a string
    pub fn load_from_str(data: &str) -> Result<Self, super::Error> {
        let mut parser = parser::Parser::new(data);
        let unit = parser.parse()?;

        Ok(unit)
    }

    /// Get an interator of values for all `key`s in all instances of `section`
    pub(crate) fn lookup_all(&self, section: &str, key: &str) -> Vec<String> {
        self.lookup_all_values(section, key)
            .iter()
            .map(|v| v.unquote())
            .collect()
    }

    pub(crate) fn lookup_all_args(&self, section: &str, key: &str) -> Vec<String> {
        self.lookup_all_values(section, key)
            .iter()
            .flat_map(|v| v.split_words())
            .collect()
    }

    /// Look up 'Environment' style key-value keys
    pub(crate) fn lookup_all_key_val(
        &self,
        section: &str,
        key: &str,
    ) -> HashMap<String, Option<String>> {
        let all_key_vals = self.lookup_all_values(section, key);

        let mut res = HashMap::with_capacity(all_key_vals.len());

        for key_vals in all_key_vals {
            for assigns in key_vals.split_words() {
                if let Some((key, value)) = assigns.split_once('=') {
                    res.insert(key.to_string(), Some(value.to_string()));
                } else {
                    res.insert(assigns, None);
                }
            }
        }

        res
    }

    pub(crate) fn lookup_all_strv(&self, section: &str, key: &str) -> Vec<String> {
        self.lookup_all_values(section, key)
            .iter()
            .flat_map(|v| v.split_strv())
            .collect()
    }

    /// Get a Vec of values for all `key`s in all instances of `section`
    /// This mimics quadlet's behavior in that empty values reset the list.
    pub(crate) fn lookup_all_values(&self, section: &str, key: &str) -> Vec<&EntryValue> {
        let values = self.lookup_all_values_raw(section, key);

        // size_hint.0 is not optimal, but may prevent forseeable growing
        let est_cap = values.size_hint().0;
        values.fold(Vec::with_capacity(est_cap), |mut res, v| {
            if v.raw().is_empty() {
                res.clear();
            } else {
                res.push(v);
            }
            res
        })
    }

    /// Get an interator of values for all `key`s in all instances of `section`
    pub(crate) fn lookup_all_values_raw(
        &self,
        section: &str,
        key: &str,
    ) -> impl DoubleEndedIterator<Item = &EntryValue> {
        self.sections
            .get(section)
            .unwrap_or_default()
            .data
            .get_all(key)
    }

    pub(crate) fn lookup(&self, section: &str, key: &str) -> Option<String> {
        self.lookup_last(section, key)
    }

    pub(crate) fn lookup_bool(&self, section: &str, key: &str) -> Option<bool> {
        self.lookup_last_value(section, key)
            .map(|v| v.to_bool().unwrap_or(false))
    }

    //TODO: lookup_int() == lookup_i64()
    //TODO: lookup_u32()
    //TODO: lookup_uid()
    //TODO: lookup_gid()

    // Get the last value for `key` in all instances of `section`
    pub(crate) fn lookup_last(&self, section: &str, key: &str) -> Option<String> {
        self.lookup_last_value(section, key).map(|v| v.unquote())
    }

    // TODO: lookup_last_args()

    // Get the last value for `key` in all instances of `section`
    pub(crate) fn lookup_last_value(&self, section: &str, key: &str) -> Option<&EntryValue> {
        self.sections
            .get(section)
            .unwrap_or_default()
            .data
            .get_all(key)
            .last()
    }

    pub(crate) fn new() -> Self {
        SystemdUnitData {
            sections: Default::default(),
        }
    }

    pub(crate) fn merge_from(&mut self, other: &SystemdUnitData) {
        for (section, entries) in other.sections.iter() {
            for (key, value) in entries.data.iter() {
                self.add_entry_value(section.clone(), key.clone(), value.clone());
            }
        }
    }

    /// Prepends `key=value` to last instance of `section`
    pub(crate) fn prepend<S, K>(&mut self, section: S, key: K, value: &str)
    where
        S: Into<String>,
        K: Into<String>,
    {
        self.prepend_entry_value(section.into(), key.into(), EntryValue::new(value));
    }

    /// Prepends `key=value` to last instance of `section`
    fn prepend_entry_value(&mut self, section: String, key: String, value: EntryValue) {
        let old_values: Vec<_> = self.sections.remove_all(&section).collect();

        self.add_entry_value(section.clone(), key.into(), value);

        for entries in old_values {
            for (ek, ev) in entries.data {
                self.add_entry_value(section.clone(), ek, ev);
            }
        }
    }

    pub(crate) fn remove_entries(&mut self, section: &str, key: &str) {
        self.sections
            .entry(section.into())
            .or_insert_entry(Entries::default())
            .into_mut()
            .data
            .remove(key);
    }

    pub(crate) fn remove_section(&mut self, section: &str) {
        self.sections.remove_all(section);
    }

    pub(crate) fn rename_section<S: Into<String>>(&mut self, from: S, to: S) {
        let from_key = from.into();

        if !self.sections.contains_key(&from_key) {
            return;
        }

        let from_aggregated_data: Vec<_> = self
            .sections
            .remove_all(&from_key)
            .flat_map(|entries| entries.data)
            .collect();

        let to_key = to.into();
        self.sections
            .entry(to_key)
            .or_insert_entry(Entries::default())
            .into_mut()
            .data
            .extend(from_aggregated_data);
    }

    pub(crate) fn section_entries<S: Into<String>>(
        &self,
        name: S,
    ) -> impl DoubleEndedIterator<Item = (&str, String)> {
        self.section_entry_values(name)
            .map(|(k, v)| (k, v.unquote()))
    }

    pub(crate) fn section_entry_values<S: Into<String>>(
        &self,
        name: S,
    ) -> impl DoubleEndedIterator<Item = (&str, &EntryValue)> {
        self.sections
            .get(&name.into())
            .unwrap_or_default()
            .data
            .iter()
            .map(|(k, v)| (k.as_str(), v))
    }

    /// Updates the last ocurrence of key to value
    pub(crate) fn set<S, K>(&mut self, section: S, key: K, value: &str)
    where
        S: Into<String>,
        K: Into<String>,
    {
        self.set_entry_value(section.into(), key.into(), EntryValue::new(value));
    }

    /// Updates the last ocurrence of key to value
    pub(crate) fn set_raw<S, K>(
        &mut self,
        section: S,
        key: K,
        value: &str,
    ) -> Result<(), super::Error>
    where
        S: Into<String>,
        K: Into<String>,
    {
        self.set_entry_value(section.into(), key.into(), EntryValue::try_from_raw(value)?);

        Ok(())
    }

    fn set_entry_value(&mut self, section: String, key: String, value: EntryValue) {
        let entries = self.sections.entry(section).or_insert(Entries::default());

        // we can't replace the last value directly, so we have to get "creative" O.o
        // we do a stupid form of read-modify-write called remove-modify-append m(
        // the good thing is: both remove() and append preserve the order of values (with this key)
        let mut values: Vec<_> = entries.data.remove_all(&key).collect();
        values.pop(); // remove the "old" last value ...

        // ... reinsert all the values again ...
        for v in values {
            entries.data.append(key.clone(), v);
        }

        // ... and append a "new" last value
        entries.data.append(key, value);
    }

    /// Write to a writer
    pub(crate) fn write_to<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        for (section, entries) in &self.sections {
            writeln!(writer, "[{}]", section)?;
            for (k, v) in &entries.data {
                writeln!(writer, "{}={}", k, v.raw())?;
            }
            writeln!(writer)?;
        }

        Ok(())
    }
}

impl Default for SystemdUnitData {
    fn default() -> Self {
        Self {
            sections: Default::default(),
        }
    }
}

impl ToString for SystemdUnitData {
    fn to_string(&self) -> String {
        let mut res = String::new();

        for (section, entries) in &self.sections {
            res.push('[');
            res.push_str(section);
            res.push_str("]\n");
            for (k, v) in &entries.data {
                res.push_str(k);
                res.push('=');
                res.push_str(v.raw());
                res.push('\n');
            }
            res.push('\n');
        }

        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod impl_default {
        use super::*;

        #[test]
        fn values() {
            let unit = SystemdUnitData::default();

            assert!(unit.sections.is_empty());
        }
    }

    mod systemd_unit {
        use super::*;

        mod add {
            use super::*;

            #[test]
            fn should_add_entry_to_known_section() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.add("Section A", "NewKey", "new value");
                assert_eq!(unit.len(), 1); // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("NewKey", "new value".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn should_create_new_section_if_necessary() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.add("New Section", "NewKey", "new value");
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("New Section");
                assert_eq!(iter.next(), Some(("NewKey", "new value".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn should_add_entry_to_last_instance_of_a_section() {
                let input = "[Section A]
KeyOne=value 1.1
KeyOne=value 1.2

[Section B]
KeyThree=value 3

[Section A]
KeyTwo=value 2
KeyOne=value 2.1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                unit.add("Section A", "KeyOne", "new value");
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.1".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.2".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 2.1".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "new value".into())));
                assert_eq!(iter.next(), None);
            }
        }

        mod add_raw {
            use super::*;

            #[test]
            fn fails_with_unqouted_value() {
                let mut unit = SystemdUnitData::new();

                let result = unit.add_raw("Section A", "KeyOne", "\\x00");

                assert!(result.is_err());
                assert_eq!(
                    result,
                    Err(crate::Error::Unquoting(
                        "\\0 character not allowed in escape sequence".into()
                    )),
                )
            }
        }

        mod has_key {
            use super::*;

            #[test]
            fn false_for_unknown_key() {
                let input = "[Section A]
KeyOne=value 1";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert!(!unit.has_key("Section A", "KeyTwo"));
                assert!(!unit.has_key("Section B", "KeyFour"));
            }

            #[test]
            fn false_for_unknown_section() {
                let input = "[Section A]
KeyOne=value 1";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert!(!unit.has_key("Section B", "KeyOne"));
            }

            #[test]
            fn true_for_key_in_section() {
                let input = "[Section A]
KeyOne=value 1

[Section B]
KeyTwo=value 2

[Section A]
KeyThree=value 1";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert!(unit.has_key("Section A", "KeyOne"));
                assert!(unit.has_key("Section B", "KeyTwo"));
                assert!(unit.has_key("Section A", "KeyThree"));
            }

            #[test]
            fn false_for_key_in_wrong_section() {
                let input = "[Section A]
KeyOne=value 1

[Section B]
KeyTwo=value 2

[Section A]
KeyThree=value 1";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert!(!unit.has_key("Section A", "KeyTwo"));
                assert!(!unit.has_key("Section B", "KeyOne"));
                assert!(!unit.has_key("Section B", "KeyThree"));
            }
        }

        mod has_section {
            use super::*;

            #[test]
            fn true_for_non_empty_section() {
                let input = "[Section A]
KeyOne=value 1";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert!(unit.has_section("Section A"));
            }

            #[test]
            fn true_for_empty_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert!(unit.has_section("Section B"));
            }

            #[test]
            fn false_for_unknown_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert!(!unit.has_section("foo"));
            }
        }

        mod len {
            use super::*;

            #[test]
            fn with_empty_file() {
                let input = "";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 0);
            }

            #[test]
            fn with_non_empty_sections() {
                let input = "[Section A]
KeyOne=value 1
[section B]
KeyTwo=value 2";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 2);
            }

            #[test]
            fn with_empty_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 2);
            }

            #[test]
            fn repeated_empty_sections_are_not_counted() {
                let input = "[Section A]
KeyOne=value 1
[Section A]
[Section B]
[Section A]
[Section C]
[Section B]";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 3);
            }

            #[test]
            fn same_section_following_itself_is_only_counted_once() {
                let input = "[Section A]
KeyOne=valueA1
[Section A]
[Section B]
KeyOne=valueB
[Section B]
KeyOne=valueB2
[Section A]
KeyOne=valueA3";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 2);
            }
        }

        mod load_from_str {
            use crate::systemd_unit::Error;

            use super::*;

            #[test]
            fn test_parsing_ignores_comments() {
                let input = "#[Section A]
#KeyOne=value 1

;[Section B]
;KeyTwo=value 2";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 0);
            }

            // NOTE: the syntax specification doesn't explicitly say this, but all sections should be named
            #[test]
            fn test_key_without_section_should_fail() {
                let input = "KeyOne=value 1";

                let result = SystemdUnitData::load_from_str(input);

                assert!(result.is_err());
                assert_eq!(
                    result,
                    Err(Error::Unit(parser::ParseError {
                        line: 0,
                        col: 1,
                        msg: "Expected comment or section".into()
                    })),
                )
            }

            #[test]
            fn trims_whitespace_after_key() {
                let input = "[Section A]
KeyOne  =value 1";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyOne", "value 1".into()));
                assert_eq!(iter.next(), None);
            }

            // NOTE: the syntax specification is silent about this case, but we'll accept it anyway
            #[test]
            fn trims_whitespace_before_key() {
                let input = "[Section A]
\tKeyOne=value 1";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyOne", "value 1".into()));
                assert_eq!(iter.next(), None);
            }

            // NOTE: the syntax specification only mentions ignoring whitespace around the '='
            #[test]
            fn trims_whitespace_around_unquoted_values() {
                let input = "[Section A]
KeyOne =    value 1
KeyTwo\t=\tvalue 2\t";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyOne", "value 1".into()));
                assert_eq!(iter.next().unwrap(), ("KeyTwo", "value 2".into()));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn trims_whitespace_around_but_not_inside_quoted_values() {
                let input = "[Section A]
KeyThree  =  \"  value 3\t\"";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyThree", "  value 3\t".into()));
                assert_eq!(iter.next(), None);
            }

            // NOTE: according to the syntax specification quotes can start at the beginning or after whitespace
            #[test]
            fn unquotes_mutiple_quotes_in_value() {
                let input = "[Section A]
Setting=\"something\" \"some thing\" \'…\'";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(
                    iter.next().unwrap(),
                    ("Setting", "something some thing …".into())
                );
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn quotes_dont_start_in_words() {
                let input = "[Section A]
Setting=foo=\"bar baz\"";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("Setting", "foo=\"bar baz\"".into()));
                assert_eq!(iter.next(), None);
            }

            // TODO: test nested quotes

            mod parse_escape_sequences {
                use super::*;

                #[test]
                fn fails_with_unknown_escape_char() {
                    let input = "[Section A]
unescape=\\_";

                    assert_eq!(
                        SystemdUnitData::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError {
                            line: 1,
                            col: 11,
                            msg:
                                "failed unquoting value: expecting escape sequence, but found '_'."
                                    .into()
                        }))
                    );
                }

                #[test]
                fn unescapes_single_character_sequences() {
                    let input = "[Section A]
unescape=\\a\\b\\f\\n\\r\\t\\v\\\\\\\"\\\'\\s";

                    let unit = SystemdUnitData::load_from_str(input).unwrap();
                    assert_eq!(unit.len(), 1);

                    let mut iter = unit.section_entries("Section A");
                    assert_eq!(
                        iter.next().unwrap(),
                        ("unescape", "\u{7}\u{8}\u{c}\n\r\t\u{b}\\\"\' ".into())
                    );
                    assert_eq!(iter.next(), None);
                }

                #[test]
                fn unescapes_unicode_sequences() {
                    let input = "[Section A]
unescape=\\xaa \\u1234 \\U0010cdef \\123";

                    let unit = SystemdUnitData::load_from_str(input).unwrap();
                    assert_eq!(unit.len(), 1);

                    let mut iter = unit.section_entries("Section A");
                    assert_eq!(
                        iter.next().unwrap(),
                        ("unescape", "\u{aa} \u{1234} \u{10cdef} \u{53}".into())
                    );
                    assert_eq!(iter.next(), None);
                }

                #[test]
                fn fails_with_escaped_null() {
                    let input = "[Section A]
unescape=\\x00";

                    assert_eq!(
                        SystemdUnitData::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 13, msg: "failed unquoting value: \\0 character not allowed in escape sequence".into()}))
                    );
                }

                #[test]
                fn fails_with_illegal_digit() {
                    let input = "[Section A]
unescape=\\u123x";

                    assert_eq!(
                        SystemdUnitData::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 15, msg: "failed unquoting value: expected 4 hex values after \"\\x\", but got \"\\x123x\"".into()}))
                    );
                }

                #[test]
                fn fails_with_illegal_octal_digit() {
                    let input = "[Section A]
unescape=\\678";

                    assert_eq!(
                        SystemdUnitData::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 13, msg: "failed unquoting value: expected 3 octal values after \"\\\", but got \"\\678\"".into()}))
                    );
                }

                #[test]
                fn fails_with_incomplete_sequence() {
                    let input = "[Section A]
unescape=\\äöü";

                    assert_eq!(
                        SystemdUnitData::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError {
                            line: 1,
                            col: 13,
                            msg:
                                "failed unquoting value: expecting escape sequence, but found 'ä'."
                                    .into()
                        }))
                    );
                }

                #[test]
                fn fails_with_incomplete_unicode_sequence() {
                    let input = "[Section A]
unescape=\\u12";

                    assert_eq!(
                        SystemdUnitData::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 13, msg: "failed unquoting value: expecting unicode escape sequence, but found EOF.".into()}))
                    );
                }
            }

            #[test]
            fn test_systemd_syntax_example_1_succeeds() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2

# a comment

[Section B]
Setting=\"something\" \"some thing\" \"…\"
KeyTwo=value 2 \\
      value 2 continued

[Section C]
KeyThree=value 3\\
# this line is ignored
; this line is ignored too
      value 3 continued";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 3);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("Section B");
                assert_eq!(
                    iter.next(),
                    Some(("Setting", "something some thing …".into()))
                );
                assert_eq!(
                    iter.next(),
                    Some(("KeyTwo", "value 2        value 2 continued".into()))
                );
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("Section C");
                assert_eq!(
                    iter.next(),
                    Some(("KeyThree", "value 3       value 3 continued".into()))
                );
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn adapted_quadlet_escapes_container_case_succeeds() {
                let input = "[Container]
Image=imagename
PodmanArgs=\"--foo\" \\
  --bar
PodmanArgs=--also
Exec=/some/path \"an arg\" \"a;b\\nc\\td'e\" a;b\\nc\\td 'a\"b'";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Container");
                assert_eq!(iter.next(), Some(("Image", "imagename".into())));
                assert_eq!(iter.next(), Some(("PodmanArgs", "--foo    --bar".into())));
                assert_eq!(iter.next(), Some(("PodmanArgs", "--also".into())));
                assert_eq!(
                    iter.next(),
                    Some((
                        "Exec",
                        "/some/path an arg a;b\nc\td'e a;b\nc\td a\"b".into()
                    ))
                );
                assert_eq!(iter.next(), None);
            }
        }

        mod lookup_all {
            use super::*;

            #[test]
            fn finds_all_across_different_instances_of_the_section() {
                let input = "[secA]
Key1=valA1.1
Key1=valA1.2
[secB]
Key1=valB1
[secA]
Key1=valA2.1
Key2=valA2";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                let values: Vec<_> = unit.lookup_all("secA", "Key1");
                assert_eq!(values, vec!["valA1.1", "valA1.2", "valA2.1"],);
            }

            #[test]
            fn finds_all_across_different_instances_of_the_section_with_reset() {
                let input = "[secA]
Key1=valA1.1
Key1=
[secB]
Key1=valB1
[secA]
Key1=valA2.1
Key2=valA2
Key1=
Key1=valA2.2
Key1=valA2.3";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                let values: Vec<_> = unit.lookup_all("secA", "Key1");
                assert_eq!(values, vec!["valA2.2", "valA2.3"],);
            }
        }

        mod lookup_all_args {
            use super::*;

            #[test]
            #[ignore]
            fn todo() {
                todo!()
            }
        }

        mod lookup_all_key_val {
            use super::*;

            #[test]
            #[ignore]
            fn todo() {
                todo!()
            }
        }

        mod lookup_all_strv {
            use super::*;

            #[test]
            #[ignore]
            fn todo() {
                todo!()
            }
        }

        mod lookup_bool {
            use super::*;

            #[test]
            #[ignore]
            fn todo() {
                todo!()
            }
        }

        mod lookup_last {
            use super::*;

            #[test]
            fn finds_last_of_multiple_values() {
                let input = "[secA]
Key1=val1
Key2=val2
Key1=val1.2";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert_eq!(unit.lookup_last("secA", "Key1"), Some("val1.2".into()),);
            }

            #[test]
            fn finds_last_when_the_last_instance_of_the_section_does_not_have_the_key() {
                let input = "[secA]
Key1=valA1
Key1=valA1.2
[secB]
Key1=valB1
[secA]
Key2=valA2";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                assert_eq!(unit.lookup_last("secA", "Key1"), Some("valA1.2".into()),);
            }
        }

        mod merge_from {
            use super::super::SystemdUnitData;

            #[test]
            fn merging_non_overlapping_section_succeeds() {
                let input_to = "[Section A]
KeyOne=value 1
KeyTwo=value 2";
                let input_from = "[New Section]
KeyOne=value 1
KeyTwo=value 2";

                let mut unit_to = SystemdUnitData::load_from_str(input_to).unwrap();
                let unit_from = SystemdUnitData::load_from_str(input_from).unwrap();

                let unchanged_section = "Section A";
                let added_section = "New Section";
                unit_to.merge_from(&unit_from);
                assert_eq!(unit_to.len(), 2);

                // newly added
                assert!(unit_to.has_section(added_section));
                let mut iter = unit_to.section_entries(added_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);

                // should not have changed
                assert!(unit_to.has_section(unchanged_section));
                let mut iter = unit_to.section_entries(unchanged_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn merging_overlapping_sections_appends_entries() {
                let input_to = "[Section A]
KeyOne=value a1
KeyTwo=value a2

[Section B]
KeyOne=value b1
KeyTwo=value b2";
                let input_from = "[New Section]
KeyOne=value 1
KeyTwo=value 2

[Section A]
KeyOne=value a1.from
KeyThree=value a3.from";

                let mut unit_to = SystemdUnitData::load_from_str(input_to).unwrap();
                let unit_from = SystemdUnitData::load_from_str(input_from).unwrap();

                let extended_section = "Section A";
                let unchanged_section = "Section B";
                let added_section = "New Section";
                unit_to.merge_from(&unit_from);
                assert_eq!(unit_to.len(), 3);

                // newly added
                assert!(unit_to.has_section(added_section));
                let mut iter = unit_to.section_entries(added_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);

                // extended with new entries
                assert!(unit_to.has_section(extended_section));
                let mut iter = unit_to.section_entries(extended_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value a1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value a2".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value a1.from".into())));
                assert_eq!(iter.next(), Some(("KeyThree", "value a3.from".into())));
                assert_eq!(iter.next(), None);

                // should not have changed
                assert!(unit_to.has_section(unchanged_section));
                let mut iter = unit_to.section_entries(unchanged_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value b1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value b2".into())));
                assert_eq!(iter.next(), None);
            }
        }

        mod prepend {
            use super::*;

            #[test]
            fn should_add_entry_to_known_section() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.prepend("Section A", "NewKey", "new value");
                assert_eq!(unit.len(), 1); // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("NewKey", "new value".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn should_create_new_section_if_necessary() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.prepend("New Section", "NewKey", "new value");
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("New Section");
                assert_eq!(iter.next(), Some(("NewKey", "new value".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn should_add_entry_to_last_instance_of_a_section() {
                let input = "[Section A]
KeyOne=value 1.1
KeyOne=value 1.2

[Section B]
KeyThree=value 3

[Section A]
KeyTwo=value 2
KeyOne=value 2.1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                unit.prepend("Section A", "KeyOne", "new value");
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "new value".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.1".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.2".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 2.1".into())));
                assert_eq!(iter.next(), None);
            }
        }

        mod remove_entries {
            use super::*;

            #[test]
            fn remove_all_entries_for_specific_key() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2
KeyOne=value 2

[Section B]
KeyOne=value 3";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                unit.remove_entries("Section A", "KeyOne");
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("Section B");
                assert_eq!(iter.next(), Some(("KeyOne", "value 3".into())));
                assert_eq!(iter.next(), None);
            }
        }

        mod remove_section {
            use super::*;

            #[test]
            fn with_single_instance_of_the_section() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2

[Section B]
KeyOne=value 3

[Section A]
KeyOne=value 4";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                unit.remove_section("Section A");
                assert_eq!(unit.len(), 1);

                assert!(!unit.has_section("Section A"));
                assert!(unit.has_section("Section B"));
            }
        }

        mod rename_section {
            use super::*;

            #[test]
            fn with_single_instance_of_the_section() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let from_section = "Section A";
                let to_section = "New Section";
                unit.rename_section(from_section, to_section);
                assert_eq!(unit.len(), 1); // shouldn't change the number of sections

                assert!(!unit.has_section(from_section));
                let mut iter = unit.section_entries(from_section);
                assert_eq!(iter.next(), None);

                assert!(unit.has_section(to_section));
                let mut iter = unit.section_entries(to_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_multiple_instances_of_a_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]
[Section A]
KeyTwo=value 2";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                let from_section = "Section A";
                let to_section = "New Section";
                unit.rename_section(from_section, to_section);
                assert_eq!(unit.len(), 2); // shouldn't change the number of sections

                assert!(!unit.has_section(from_section));
                let mut iter = unit.section_entries(from_section);
                assert_eq!(iter.next(), None);

                assert!(unit.has_section(to_section));
                let mut iter = unit.section_entries(to_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_unknown_section_should_do_anything() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let from_section = "foo";
                let to_section = "New";
                let other_section = "Section A";

                assert!(!unit.has_section(from_section));
                unit.rename_section(from_section, to_section);
                assert_eq!(unit.len(), 1); // shouldn't change the number of sections

                assert!(unit.has_section(other_section));
                let mut iter = unit.section_entries(other_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);

                assert!(!unit.has_section(to_section));
                let mut iter = unit.section_entries(to_section);
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn keeps_entries_already_present_in_destination_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]
KeyTwo=value 2
[Section A]
KeyThree=value 3";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                let from_section = "Section A";
                let to_section = "Section B";
                unit.rename_section(from_section, to_section);
                assert_eq!(unit.len(), 1);

                assert!(!unit.has_section(from_section));
                let mut iter = unit.section_entries(from_section);
                assert_eq!(iter.next(), None);

                assert!(unit.has_section(to_section));
                let mut iter = unit.section_entries(to_section);
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyThree", "value 3".into())));
                assert_eq!(iter.next(), None);
            }
        }

        mod section_entries {
            use super::*;

            #[test]
            fn with_one_section() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn test_with_same_section_occuring_multiple_times() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2

[Section B]
Key=value

[Section A]
KeyOne=value 1.2";

                let unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.2".into())));
                assert_eq!(iter.next(), None);
            }
        }

        mod set {
            use super::*;

            #[test]
            fn adds_entry_to_new_section() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set("Section B", "KeyTwo", "value 2");
                assert_eq!(unit.len(), 2); // should have added new section

                // unchanged
                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), None);

                // added
                let mut iter = unit.section_entries("Section B");
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn adds_entry_with_new_key() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set("Section A", "KeyTwo", "value 2");
                assert_eq!(unit.len(), 1); // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn replaces_entry_with_same_key_in_section() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set("Section A", "KeyOne", "new value");
                assert_eq!(unit.len(), 1); // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "new value".into())));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn replaces_last_entry_with_same_key_in_section() {
                let input = "[Section A]
KeyOne=value 1
KeyOne=value 2
KeyOne=value 3";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set("Section A", "KeyOne", "new value");
                assert_eq!(unit.len(), 1); // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "value 2".into())));
                assert_eq!(iter.next(), Some(("KeyOne", "new value".into())));
                assert_eq!(iter.next(), None);
            }
        }

        mod round_trip {
            use super::*;

            #[test]
            fn read_write_round_trip_without_modifications() {
                let input = "[Service]
ExecStart=/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'";

                let unit = SystemdUnitData::load_from_str(input).unwrap();

                let exec_start =
                    unit.lookup_last_value(crate::systemd_unit::SERVICE_SECTION, "ExecStart");
                assert_eq!(
                    exec_start.map(|ev| ev.raw().as_str()),
                    Some("/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'")
                );
                assert_eq!(
                    exec_start.map(|ev| ev.to_string()),
                    Some("/some/path an arg a;b\nc\td\'e a;b\nc\td a\"b".into())
                );

                let mut output = Vec::new();
                let res = unit.write_to(&mut output);
                assert!(res.is_ok());

                assert_eq!(
                    // NOTE: we trim here, because `write_to()` ends the file in \n
                    std::str::from_utf8(&output).unwrap().trim_end(),
                    input,
                );
            }

            #[test]
            fn with_word_splitting_and_setting_constructed_command() {
                use crate::quadlet::podman_command::PodmanCommand;

                let input = "[Service]
ExecStart=/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'";

                let mut unit = SystemdUnitData::load_from_str(input).unwrap();

                let exec_start =
                    unit.lookup_last_value(crate::systemd_unit::SERVICE_SECTION, "ExecStart");
                assert_eq!(
                    exec_start.map(|ev| ev.raw().as_str()),
                    Some("/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'")
                );
                assert_eq!(
                    exec_start.map(|ev| ev.unquote()),
                    Some("/some/path an arg a;b\nc\td\'e a;b\nc\td a\"b".into())
                );

                let split_words: Vec<String> = exec_start.unwrap().split_words().collect();
                let mut split = split_words.iter();
                assert_eq!(split.next(), Some(&"/some/path".into()));
                assert_eq!(split.next(), Some(&"an arg".into()));
                assert_eq!(split.next(), Some(&"a;b\nc\td\'e".into()));
                assert_eq!(split.next(), Some(&"a;b\nc\td".into()));
                assert_eq!(split.next(), Some(&"a\"b".into()));
                assert_eq!(split.next(), None);

                let mut command = PodmanCommand::new();
                command.add("test");
                command.extend(split_words.into_iter());

                let new_exec_start = command.to_escaped_string();
                assert_eq!(
                    new_exec_start,
                    "/usr/bin/podman test /some/path \"an arg\" \"a;b\\nc\\td\'e\" \"a;b\\nc\\td\" \"a\\\"b\""
                );

                let _ = unit.set_raw(
                    crate::systemd_unit::SERVICE_SECTION,
                    "ExecStart",
                    new_exec_start.as_str(),
                );

                let mut output = Vec::new();
                let res = unit.write_to(&mut output);
                assert!(res.is_ok());

                assert_eq!(
                    std::str::from_utf8(&output).unwrap(),
                    "[Service]\nExecStart=/usr/bin/podman test /some/path \"an arg\" \"a;b\\nc\\td'e\" \"a;b\\nc\\td\" \"a\\\"b\"\n\n",
                );
            }
        }

        mod to_string {
            use super::*;

            #[test]
            fn with_empty_unit() {
                let unit = SystemdUnitData::new();

                assert_eq!(unit.to_string(), "");
            }

            #[test]
            fn with_basic_entries() {
                let mut unit = SystemdUnitData::new();

                unit.set("Section A", "KeyOne", "value 1");
                unit.set("Section B", "KeyTwo", "value 2");
                unit.set("Section B", "KeyThree", "value\n3");

                assert_eq!(
                    unit.to_string(),
                    "[Section A]
KeyOne=value 1

[Section B]
KeyTwo=value 2
KeyThree=value\\n3

"
                );
            }

            #[test]
            fn with_raw_entries() {
                let mut unit = SystemdUnitData::new();

                unit.set("Section A", "KeyOne", "value 1");
                unit.set("Section B", "KeyTwo", "\"value 2\"");
                let _ = unit.set_raw("Section B", "KeyThree", "\"value 3\"");

                assert_eq!(
                    unit.to_string(),
                    "[Section A]
KeyOne=value 1

[Section B]
KeyTwo=\\\"value 2\\\"
KeyThree=\"value 3\"

"
                );
            }
        }
    }
}
