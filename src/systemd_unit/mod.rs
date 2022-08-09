mod constants;
mod parser;
mod quoted;
mod split;
mod value;

use crate::quadlet::IdRanges;

pub use self::constants::*;
pub use self::quoted::*;
pub use self::split::*;
use self::value::*;

use nix::unistd::{Gid, Uid, User, Group};
use std::fmt;
use std::io;
use std::path::PathBuf;
use ordered_multimap::list_ordered_multimap::ListOrderedMultimap;

// TODO: mimic https://doc.rust-lang.org/std/num/enum.IntErrorKind.html
// TODO: use thiserror?
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Error {
    ParseBool,
    Unit(parser::ParseError),
    Gid(nix::errno::Errno),
    Uid(nix::errno::Errno),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ParseBool => {
                write!(f, "value must be one of `1`, `yes`, `true`, `on`, `0`, `no`, `false`, `off`")
            },
            Error::Unit(e) => {
                write!(f, "failed to parse unit file: {e}")
            },
            Error::Gid(e) => {
                write!(f, "failed to parse group name/id: {e}")
            },
            Error::Uid(e) => {
                write!(f, "failed to parse user name/id: {e}")
            },
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
        return Ok(false)
    }

    Err(Error::ParseBool)
}

pub(crate) fn parse_gid(s: &str) -> Result<Gid, Error> {
    match s.parse::<u32>() {
        Ok(uid) => return Ok(Gid::from_raw(uid)),
        Err(_) => (),
    }

    return match Group::from_name(s) {
        Ok(g) => return Ok(g.unwrap().gid),
        Err(e) => Err(Error::Gid(e)),
    }
}

/// Parses subuids/subgids for remapping.
/// Inputs can have the form of a user name or id ranges (separated by ',').
/// Ranges can be "open" (i.e. only have a start value). In that case the end
/// value will default to the maximum allowed id value.
///
/// see also the documentation for the `RemapUidRanges` and `RemapGidRanges` fields.
///
/// NOTE: Looking up id ranges for user names needs a lookup function (i.e. `name_lookup`)
/// that can turn a user name into a range of ids (e.g by parsing _/etc/sub*uid_).
/// Quadlet-rs has such functions already.
/// If you don't need this, you can provide `|_| None` which will map all user names
/// to an empty set of id ranges.
///
/// valid inputs are:
/// - a username (e.g. "quadlet") in combination with a lookup function
/// - a range of ids (e.g. "100000-101000")
/// - multiple ranges (e.g. "1000-2000,100000-101000")
/// - an "open" range (e.g. "100000"). The end will default to the maximum allowed id value.
pub(crate) fn parse_ranges<F>(s: &str, name_lookup: F) -> IdRanges
    where F: Fn(&str) -> Option<IdRanges>
{
    if s.is_empty() {
        return IdRanges::empty()
    }

    if !s.chars().next().unwrap().is_ascii_digit() {
        return name_lookup(s).unwrap_or(IdRanges::empty())
    }

    IdRanges::parse(s)
}

pub(crate) fn parse_uid(s: &str) -> Result<Uid, Error> {
    match s.parse::<u32>() {
        Ok(uid) => return Ok(Uid::from_raw(uid)),
        Err(_) => (),
    }

    return match User::from_name(s) {
        Ok(u) => return Ok(u.unwrap().uid),
        Err(e) => Err(Error::Uid(e)),
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct SystemdUnit {
    pub(crate) path: Option<PathBuf>,
    sections: ListOrderedMultimap<SectionKey, Entries>,
}

impl SystemdUnit {
    /// Appends `key=value` to last instance of `section`
    pub(crate) fn append_entry<S, K, V>(&mut self, section: S, key: K, value: V)
    where S: Into<String>,
          K: Into<String>,
          V: Into<String>,
    {
        let value = value.into();

        self.append_entry_value(
            section,
            key,
            EntryValue {
                raw: quote_value(value.as_str()),
                unquoted: value,
            }
        );
    }
    /// Appends `key=value` to last instance of `section`
    pub(crate) fn append_entry_value<S, K>(&mut self, section: S, key: K, value: EntryValue)
    where S: Into<String>,
          K: Into<String>,
    {
        self.sections
            .entry(section.into())
            .or_insert_entry(Entries::default())
            .into_mut()
            .data.append(key.into(), value);
    }

    pub(crate) fn has_key<S, K>(&self, section: S, key: K) -> bool
    where S: Into<String>,
          K: Into<String>,
    {
        self.sections
            .get(&section.into())
            .map_or(false, |e| e.data.contains_key(&key.into()))
    }

    /// Retrun `true` if there's an (non-empty) instance of section `name`
    pub(crate) fn has_section<S: Into<String>>(&self, name: S) -> bool {
        self.sections.contains_key(&name.into())
    }

    /// Number of unique sections (i.e. with different names)
    pub fn len(&self) -> usize {
        self.sections.keys_len()
    }

    /// Load from a string
    pub fn load_from_str(data: &str) -> Result<Self, Error> {
        let mut parser = parser::Parser::new(data);
        let unit = parser.parse()?;

        Ok(unit)
    }

    /// Get an interator of values for all `key`s in all instances of `section`
    pub(crate) fn lookup_all<'a, S, K>(&'a self, section: S, key: K) -> impl DoubleEndedIterator<Item = &'a str>
    where S: Into<String>,
          K: Into<String>,
    {
        self.lookup_all_values(section, key)
            .map(|v| v.unquoted().as_str())
    }

    /// Get an interator of values for all `key`s in all instances of `section`
    pub(crate) fn lookup_all_values<'a, S, K>(&'a self, section: S, key: K) -> impl DoubleEndedIterator<Item = &EntryValue>
    where S: Into<String>,
          K: Into<String>,
    {
        self.sections
            .get(&section.into())
            .unwrap_or_default()
            .data
            .get_all(&key.into())
            .map(|v| v)
    }

    /// Get a Vec of values for all `key`s in all instances of `section`
    /// This mimics quadlet's behavior in that empty values reset the list.
    pub(crate) fn lookup_all_with_reset<'a, S, K>(&'a self, section: S, key: K) -> Vec<&'a str>
    where S: Into<String>,
          K: Into<String>,
    {
        let values = self.sections
            .get(&section.into())
            .unwrap_or_default()
            .data.get_all(&key.into())
            .map(|v| v.unquoted().as_str());

        // size_hint.0 is not optimal, but may prevent forseeable growing
        let est_cap = values.size_hint().0;
        values.fold( Vec::with_capacity(est_cap), |mut res, v| {
            if v.is_empty() {
                res.clear();
            } else {
                res.push(v);
            }
            res
        })
    }

    // Get the last value for `key` in all instances of `section`
    pub(crate) fn lookup_last<'a, S, K>(&'a self, section: S, key: K) -> Option<&'a str>
    where S: Into<String>,
          K: Into<String>,
    {
        self.lookup_last_value(section, key)
            .map(|v| v.unquoted().as_str())
    }

    // Get the last value for `key` in all instances of `section`
    pub(crate) fn lookup_last_value<'a, S, K>(&'a self, section: S, key: K) -> Option<&EntryValue>
    where S: Into<String>,
          K: Into<String>,
    {
        self.sections
            .get(&section.into())
            .unwrap_or_default()
            .data
            .get_all(&key.into())
            .last()
    }

    pub(crate) fn new() -> Self {
        SystemdUnit {
            path: None,
            sections: Default::default(),
        }
    }

    pub(crate) fn merge_from(&mut self, other: &SystemdUnit) {
        for (section, props) in other.inner.iter() {
            match self.inner.entry(section.map(|s| s.to_string())) {
                ini::SectionEntry::Vacant(se) => { se.insert(props.clone()); },
                ini::SectionEntry::Occupied(mut se) => se.append(props.clone()),
            };
        }
    }

    pub(crate) fn path(&self) -> &Option<PathBuf> {
        &self.path
    }

    pub(crate) fn rename_section<S: Into<String>>(&mut self, from: S, to: S) {
        let from_key = from.into();

        if !self.sections.contains_key(&from_key) {
            return
        }

        let from_values: Vec<Entries> = self.sections.remove_all(&from_key).collect();

        if from_values.is_empty() {
            return
        }

        let to_key = to.into();
        for entries in from_values {
            for (ek, ev) in entries.data {
                self.append_entry_value(to_key.clone(), ek, ev);
            }
        }
    }

    pub(crate) fn section_entries<'a, S: Into<String>>(&'a self, name: S) -> impl DoubleEndedIterator<Item=(&'a str, &'a str)> {
        self.section_entry_values(name)
            .map(|(k, v)| (k, v.unquoted().as_str()))
    }

    pub(crate) fn section_entry_values<'a, S: Into<String>>(&'a self, name: S) -> impl DoubleEndedIterator<Item=(&'a str, &EntryValue)> {
        self.sections
            .get(&name.into())
            .unwrap_or_default()
            .data
            .iter()
            .map(|(k, v)| (k.as_str(), v))
    }

    pub(crate) fn set_entry<S, K, V>(&mut self, section: S, key: K, value: V)
    where S: Into<String>,
          K: Into<String>,
          V: Into<String>,
    {
        let value = value.into();

        self.set_entry_value(
            section,
            key,
            EntryValue {
                raw: quote_value(value.as_str()),
                unquoted: value,
            }
        );
    }

    pub(crate) fn set_entry_value<S, K>(&mut self, section: S, key: K, value: EntryValue)
    where S: Into<String>,
          K: Into<String>,
    {
        let entries = self.sections
            .entry(section.into())
            .or_insert(Entries::default());

        let key = key.into();

        // we can't replace the last value directly, so we have to get "creative" O.o
        // we do a stupid form of read-modify-write called remove-modify-append m(
        // the good thing is: both remove() and append preserve the order of values (with this key)
        let mut values: Vec<_> = entries.data.remove_all(&key).collect();
        values.pop();  // remove the "old" last value ...
        // ... reinsert all the values again ...
        for v in values {
            entries.data.append(key.clone(), v);

        }
        // ... and append a "new" last value
        entries.data.append(key.into(), value);

    }

    /// Write to a writer
    pub(crate) fn write_to<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        self.inner.write_to_opt(
            writer,
            WriteOption {
                escape_policy: EscapePolicy::Basics,
                ..WriteOption::default()
            },
        )
    }
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
        fn false_with_falthy_input() {
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

    mod parse_gid {
        use nix::errno::Errno;

        use super::*;

        #[test]
        fn fails_with_empty_input() {
            let input = "";

            let res = parse_gid(input);
            assert_eq!(res.err(), Some(Error::Gid(Errno::ENOENT)));
        }

        #[test]
        fn parses_integer_gid() {
            let input = "12345";

            let res = parse_gid(input);
            assert_eq!(res.ok(), Some(Gid::from_raw(12345)));
        }

        #[test]
        fn fails_parsing_integer_with_gunk() {
            let input = "12345%";

            let res = parse_gid(input);
            assert_eq!(res.err(), Some(Error::Gid(Errno::ENOENT)));
        }

        #[test]
        fn converts_group_name() {
            let input = "root";

            let res = parse_gid(input);
            assert_eq!(res.ok(), Some(Gid::from_raw(0)));
        }

        #[test]
        fn converts_group_name2() {
            let input = User::from_name("mail")
                .expect("should have this group")
                .expect("should have this group");

            let res = parse_gid(input.name.as_str());
            assert_eq!(res.ok(), Some(input.gid));
        }
    }

    mod parse_ranges {
        use crate::quadlet::IdMap;
        use super::*;

        #[test]
        fn empty_range_with_empty_input() {
            let input = "";

            let res = parse_ranges(input, |_| None);
            assert!(res.is_empty());
        }

        #[test]
        fn uses_name_lookup_for_user_name() {
            let input = "quadlet";

            let ranges = parse_ranges(input, |_| Some(IdRanges::new(123, 456)));

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), Some(IdMap::new(123, 456)));
            assert_eq!(iter.next(), None)
        }

        #[test]
        fn name_lookup_falls_back_to_empty_range() {
            let input = "quadlet";

            let ranges = parse_ranges(input, |_| None);

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), None)
        }

        #[test]
        fn defaults_to_empty_range_without_lookup_function() {
            let input = "quadlet";

            let ranges = parse_ranges(input, |_| None);

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), None)
        }

        #[test]
        fn with_single_number() {
            let input = "123";

            let ranges = parse_ranges(input, |_| None);

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), Some(IdMap::new(123, u32::MAX-123)));
            assert_eq!(iter.next(), None)
        }

        #[test]
        fn with_single_numeric_range() {
            let input = "123-456";

            let ranges = parse_ranges(input, |_| None);

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), Some(IdMap::new(123, 334)));
            assert_eq!(iter.next(), None)
        }

        #[test]
        fn with_numeric_range_and_number() {
            let input = "123-456,789";

            let ranges = parse_ranges(input, |_| None);

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), Some(IdMap::new(123, 334)));
            assert_eq!(iter.next(), Some(IdMap::new(789, u32::MAX-789)));
            assert_eq!(iter.next(), None)
        }

        #[test]
        fn with_multiple_numeric_ranges() {
            let input = "123-456,789-101112";

            let ranges = parse_ranges(input, |_| None);

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), Some(IdMap::new(123, 334)));
            assert_eq!(iter.next(), Some(IdMap::new(789, 100324)));
            assert_eq!(iter.next(), None)
        }

        #[test]
        fn merges_overlapping_non_monotonic_numeric_ranges() {
            let input = "123-456,345,234-567";

            let ranges = parse_ranges(input, |_| None);

            let mut iter = ranges.iter();
            assert_eq!(iter.next(), Some(IdMap::new(123, u32::MAX-123)));
            assert_eq!(iter.next(), None)
        }
    }

    mod parse_uid {
        use nix::errno::Errno;

        use super::*;

        #[test]
        fn fails_with_empty_input() {
            let input = "";

            let res = parse_uid(input);
            assert_eq!(res.err(), Some(Error::Uid(Errno::ENOENT)));
        }

        #[test]
        fn parses_integer_uid() {
            let input = "12345";

            let res = parse_uid(input);
            assert_eq!(res.ok(), Some(Uid::from_raw(12345)));
        }

        #[test]
        fn fails_parsing_integer_with_gunk() {
            let input = "12345%";

            let res = parse_uid(input);
            assert_eq!(res.err(), Some(Error::Uid(Errno::ENOENT)));
        }

        #[test]
        fn converts_user_name() {
            let input = "root";

            let res = parse_uid(input);
            assert_eq!(res.ok(), Some(Uid::from_raw(0)));
        }

        #[test]
        fn converts_user_name2() {
            let input = User::from_name("mail")
                .expect("should have this user")
                .expect("should have this user");

            let res = parse_uid(input.name.as_str());
            assert_eq!(res.ok(), Some(input.uid));
        }
    }

    mod systemd_unit {
        use super::*;

        mod add_entry {
            use super::*;

            #[test]
            fn should_add_entry_to_known_section() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.append_entry("Section A", "NewKey", "new value");
                assert_eq!(unit.len(), 1);  // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("NewKey", "new value")));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn should_create_new_section_if_necessary() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.append_entry("New Section", "NewKey", "new value");
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("New Section");
                assert_eq!(iter.next(), Some(("NewKey", "new value")));
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

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                unit.append_entry("Section A", "KeyOne", "new value");
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.1")));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.2")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), Some(("KeyOne", "value 2.1")));
                assert_eq!(iter.next(), Some(("KeyOne", "new value")));
                assert_eq!(iter.next(), None);
            }
        }

        mod has_key {
            use super::*;

            #[test]
            fn false_for_unknown_key() {
                let input = "[Section A]
KeyOne=value 1";

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert!(!unit.has_key("Section A", "KeyTwo"));
                assert!(!unit.has_key("Section B", "KeyFour"));
            }

            #[test]
            fn false_for_unknown_section() {
                let input = "[Section A]
KeyOne=value 1";

                let unit = SystemdUnit::load_from_str(input).unwrap();

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

                let unit = SystemdUnit::load_from_str(input).unwrap();

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

                let unit = SystemdUnit::load_from_str(input).unwrap();

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

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert!(unit.has_section("Section A"));
            }

            #[test]
            fn true_for_empty_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]";

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert!(unit.has_section("Section B"));
            }

            #[test]
            fn false_for_unknown_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]";

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert!(!unit.has_section("foo"));
            }
        }

        mod len {
            use super::*;

            #[test]
            fn with_empty_file() {
                let input = "";

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 0);
            }

            #[test]
            fn with_non_empty_sections() {
                let input = "[Section A]
KeyOne=value 1
[section B]
KeyTwo=value 2";

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 2);
            }

            #[test]
            fn with_empty_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]";

                let unit = SystemdUnit::load_from_str(input).unwrap();

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

                let unit = SystemdUnit::load_from_str(input).unwrap();

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

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert_eq!(unit.len(), 2);
            }
        }

        mod load_from_str {
            use super::*;

            #[test]
            fn test_parsing_ignores_comments() {
                let input = "#[Section A]
#KeyOne=value 1

;[Section B]
;KeyTwo=value 2";

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 0);
            }

            // NOTE: the syntax specification doesn't explicitly say this, but all sections should be named
            #[test]
            fn test_key_without_section_should_fail() {
                let input = "KeyOne=value 1";

                let result = SystemdUnit::load_from_str(input);

                assert!(result.is_err());
                assert_eq!(
                    result,
                    Err(Error::Unit(parser::ParseError{ line: 0, col: 1, msg: "Expected comment or section".into() })),
                )
            }

            #[test]
            fn trims_whitespace_after_key() {
                let input = "[Section A]
KeyOne  =value 1";

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyOne", "value 1"));
                assert_eq!(iter.next(), None);
            }

            // NOTE: the syntax specification is silent about this case, but we'll accept it anyway
            #[test]
            fn trims_whitespace_before_key() {
                let input = "[Section A]
\tKeyOne=value 1";

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyOne", "value 1"));
                assert_eq!(iter.next(), None);
            }

            // NOTE: the syntax specification only mentions ignoring whitespace around the '='
            #[test]
            fn trims_whitespace_around_unquoted_values() {
                let input = "[Section A]
KeyOne =    value 1
KeyTwo\t=\tvalue 2\t";

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyOne", "value 1"));
                assert_eq!(iter.next().unwrap(), ("KeyTwo", "value 2\t"));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn trims_whitespace_around_but_not_inside_quoted_values() {
                let input = "[Section A]
KeyThree  =  \"  value 3\t\"";

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("KeyThree", "  value 3\t"));
                assert_eq!(iter.next(), None);
            }

            // NOTE: according to the syntax specification quotes can start at the beginning or after whitespace
            #[test]
            fn unquotes_mutiple_quotes_in_value() {
                let input = "[Section A]
Setting=\"something\" \"some thing\" \'…\'";

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("Setting", "something some thing …"));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn quotes_dont_start_in_words() {
                let input = "[Section A]
Setting=foo=\"bar baz\"";

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next().unwrap(), ("Setting", "foo=\"bar baz\""));
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
                        SystemdUnit::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 11, msg: "expecting escape sequence, but found '_'.".into()}))
                    );
                }

                #[test]
                fn unescapes_single_character_sequences() {
                    let input = "[Section A]
unescape=\\a\\b\\f\\n\\r\\t\\v\\\\\\\"\\\'\\s";

                    let unit = SystemdUnit::load_from_str(input).unwrap();
                    assert_eq!(unit.len(), 1);

                    let mut iter = unit.section_entries("Section A");
                    assert_eq!(iter.next().unwrap(), ("unescape", "\u{7}\u{8}\u{c}\n\r\t\u{b}\\\"\' "));
                    assert_eq!(iter.next(), None);
                }

                #[test]
                fn unescapes_unicode_sequences() {
                    let input = "[Section A]
unescape=\\xaa \\u1234 \\U0010cdef \\123";

                    let unit = SystemdUnit::load_from_str(input).unwrap();
                    assert_eq!(unit.len(), 1);

                    let mut iter = unit.section_entries("Section A");
                    assert_eq!(iter.next().unwrap(), ("unescape", "\u{aa} \u{1234} \u{10cdef} \u{53}"));
                    assert_eq!(iter.next(), None);
                }

                #[test]
                fn fails_with_escaped_null() {
                    let input = "[Section A]
unescape=\\x00";

                    assert_eq!(
                        SystemdUnit::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 13, msg: "\\0 character not allowed in escape sequence".into()}))
                    );
                }

                #[test]
                fn fails_with_illegal_digit() {
                    let input = "[Section A]
unescape=\\u123x";

                    assert_eq!(
                        SystemdUnit::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 15, msg: "Expected 4 hex values after \"\\x\", but got \"\\x123x\"".into()}))
                    );
                }

                #[test]
                fn fails_with_illegal_octal_digit() {
                    let input = "[Section A]
unescape=\\678";

                    assert_eq!(
                        SystemdUnit::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 13, msg: "Expected 3 octal values after \"\\\", but got \"\\678\"".into()}))
                    );
                }

                #[test]
                fn fails_with_incomplete_sequence() {
                    let input = "[Section A]
unescape=\\äöü";

                    assert_eq!(
                        SystemdUnit::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{line: 1, col: 13, msg: "expecting escape sequence, but found 'ä'.".into()}))
                    );
                }

                #[test]
                fn fails_with_incomplete_unicode_sequence() {
                    let input = "[Section A]
unescape=\\u12";

                    assert_eq!(
                        SystemdUnit::load_from_str(input).err(),
                        Some(Error::Unit(parser::ParseError{ line: 1, col: 13, msg: "expecting unicode escape sequence, but found EOF.".into()}))
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

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 3);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("Section B");
                assert_eq!(iter.next(), Some(("Setting", "something some thing …")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2        value 2 continued")));
                assert_eq!(iter.next(), None);

                let mut iter = unit.section_entries("Section C");
                assert_eq!(iter.next(), Some(("KeyThree", "value 3       value 3 continued")));
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

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Container");
                assert_eq!(iter.next(), Some(("Image", "imagename")));
                assert_eq!(iter.next(), Some(("PodmanArgs", "--foo    --bar")));
                assert_eq!(iter.next(), Some(("PodmanArgs", "--also")));
                assert_eq!(iter.next(), Some(("Exec", "/some/path an arg a;b\nc\td'e a;b\nc\td a\"b")));
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

                let unit = SystemdUnit::load_from_str(input).unwrap();

                let values: Vec<_> = unit.lookup_all("secA", "Key1").collect();
                assert_eq!(
                    values,
                    vec!["valA1.1", "valA1.2", "valA2.1"],
                );
            }
        }

        mod lookup_all_with_reset{
            use super::*;

            #[test]
            fn finds_all_across_different_instances_of_the_section() {
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

                let unit = SystemdUnit::load_from_str(input).unwrap();

                let values: Vec<_> = unit.lookup_all_with_reset("secA", "Key1");
                assert_eq!(
                    values,
                    vec!["valA2.2", "valA2.3"],
                );
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

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert_eq!(
                    unit.lookup_last("secA", "Key1"),
                    Some("val1.2"),
                );
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

                let unit = SystemdUnit::load_from_str(input).unwrap();

                assert_eq!(
                    unit.lookup_last("secA", "Key1"),
                    Some("valA1.2"),
                );
            }
        }

        mod rename_section {
            use super::*;

            #[test]
            fn with_single_instance_of_the_section() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let from_section = "Section A";
                let to_section = "New Section";
                unit.rename_section(from_section, to_section);
                assert_eq!(unit.len(), 1);  // shouldn't change the number of sections

                assert!(!unit.has_section(from_section));
                let mut iter = unit.section_entries(from_section);
                assert_eq!(iter.next(), None);

                assert!(unit.has_section(to_section));
                let mut iter = unit.section_entries(to_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_multiple_instances_of_a_section() {
                let input = "[Section A]
KeyOne=value 1
[Section B]
[Section A]
KeyTwo=value 2";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                let from_section = "Section A";
                let to_section = "New Section";
                unit.rename_section(from_section, to_section);
                assert_eq!(unit.len(), 2);  // shouldn't change the number of sections

                assert!(!unit.has_section(from_section));
                let mut iter = unit.section_entries(from_section);
                assert_eq!(iter.next(), None);

                assert!(unit.has_section(to_section));
                let mut iter = unit.section_entries(to_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn with_unknown_section_should_do_anything() {
                let input = "[Section A]
KeyOne=value 1
KeyTwo=value 2";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let from_section = "foo";
                let to_section = "New";
                let other_section = "Section A";

                assert!(!unit.has_section(from_section));
                unit.rename_section(from_section, to_section);
                assert_eq!(unit.len(), 1);  // shouldn't change the number of sections

                assert!(unit.has_section(other_section));
                let mut iter = unit.section_entries(other_section);
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
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

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
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
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyThree", "value 3")));
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

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
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

                let unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 2);

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), Some(("KeyOne", "value 1.2")));
                assert_eq!(iter.next(), None);
            }
        }

        mod set_entry {
            use super::*;

            #[test]
            fn adds_entry_to_new_section() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set_entry("Section B", "KeyTwo", "value 2");
                assert_eq!(unit.len(), 2);  // should have added new section

                // unchanged
                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), None);

                // added
                let mut iter = unit.section_entries("Section B");
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn adds_entry_with_new_key() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set_entry("Section A", "KeyTwo", "value 2");
                assert_eq!(unit.len(), 1);  // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyTwo", "value 2")));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn replaces_entry_with_same_key_in_section() {
                let input = "[Section A]
KeyOne=value 1";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set_entry("Section A", "KeyOne", "new value");
                assert_eq!(unit.len(), 1);  // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "new value")));
                assert_eq!(iter.next(), None);
            }

            #[test]
            fn replaces_last_entry_with_same_key_in_section() {
                let input = "[Section A]
KeyOne=value 1
KeyOne=value 2
KeyOne=value 3";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();
                assert_eq!(unit.len(), 1);

                unit.set_entry("Section A", "KeyOne", "new value");
                assert_eq!(unit.len(), 1);  // shouldn't change the number of sections

                let mut iter = unit.section_entries("Section A");
                assert_eq!(iter.next(), Some(("KeyOne", "value 1")));
                assert_eq!(iter.next(), Some(("KeyOne", "value 2")));
                assert_eq!(iter.next(), Some(("KeyOne", "new value")));
                assert_eq!(iter.next(), None);
            }
        }

        mod round_trip {
            use crate::quadlet::PodmanCommand;

            use super::*;

            #[test]
            fn read_write_round_trip_without_modifications() {
                let input = "[Service]
ExecStart=/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'";

                let unit = SystemdUnit::load_from_str(input).unwrap();

                let exec_start = unit.lookup_last_value(SERVICE_SECTION, "ExecStart");
                assert_eq!(
                    exec_start.map(|ev| ev.raw().as_str()),
                    Some("/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'")
                );
                assert_eq!(
                    exec_start.map(|ev| ev.unquoted().as_str()),
                    Some("/some/path an arg a;b\nc\td\'e a;b\nc\td a\"b")
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
                let input = "[Service]
ExecStart=/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'";

                let mut unit = SystemdUnit::load_from_str(input).unwrap();

                let exec_start = unit.lookup_last_value(SERVICE_SECTION, "ExecStart");
                assert_eq!(
                    exec_start.map(|ev| ev.raw().as_str()),
                    Some("/some/path \"an arg\" \"a;b\\nc\\td\'e\" a;b\\nc\\td \'a\"b\'")
                );
                assert_eq!(
                    exec_start.map(|ev| ev.unquoted().as_str()),
                    Some("/some/path an arg a;b\nc\td\'e a;b\nc\td a\"b")
                );

                let mut split_words: Vec<String> = SplitWord::new(exec_start.unwrap().raw()).collect();
                let mut split = split_words.iter();
                assert_eq!(split.next(), Some(&"/some/path".into()));
                assert_eq!(split.next(), Some(&"an arg".into()));
                assert_eq!(split.next(), Some(&"a;b\nc\td\'e".into()));
                assert_eq!(split.next(), Some(&"a;b\nc\td".into()));
                assert_eq!(split.next(), Some(&"a\"b".into()));
                assert_eq!(split.next(), None);

                let mut command = PodmanCommand::new_command("test");
                command.add_vec(&mut split_words);

                let new_exec_start = command.to_escaped_string();
                assert_eq!(
                    new_exec_start,
                    "/usr/bin/podman test /some/path \"an arg\" \"a;b\nc\td\'e\" \"a;b\nc\td\" \"a\\\"b\""
                );

                unit.set_entry(SERVICE_SECTION, "ExecStart", new_exec_start.as_str());

                let mut output = Vec::new();
                let res = unit.write_to(&mut output);
                assert!(res.is_ok());

                assert_eq!(
                    std::str::from_utf8(&output).unwrap(),
                    "[Service]\nExecStart=/usr/bin/podman test /some/path \"an arg\" \"a;b\\nc\\td'e\" \"a;b\\nc\\td\" \"a\\\"b\"\n",
                );
            }
        }
    }
}