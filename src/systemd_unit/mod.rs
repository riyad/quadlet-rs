mod constants;
mod split;

use crate::quadlet::IdRanges;

pub use self::constants::*;
pub use self::split::*;

use ini::{Ini, ParseOption};
use nix::unistd::{Gid, Uid, User, Group};
use std::fmt;
use std::io;
use std::path::{PathBuf, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    Bool,
    Gid(nix::errno::Errno),
    Uid(nix::errno::Errno),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Bool => {
                write!(f, "value must be one of `1`, `yes`, `true`, `on`, `0`, `no`, `false`, `off`")
            },
            ParseError::Gid(e) => {
                write!(f, "failed to parse group name/id: {e}")
            },
            ParseError::Uid(e) => {
                write!(f, "failed to parse user name/id: {e}")
            },
        }
    }
}

pub(crate) fn parse_bool(s: &str) -> Result<bool, ParseError> {
    if ["1", "yes", "true", "on"].contains(&s) {
        return Ok(true);
    } else if ["0", "no", "false", "off"].contains(&s) {
        return Ok(false)
    }

    Err(ParseError::Bool)
}

pub(crate) fn parse_gid(s: &str) -> Result<Gid, ParseError> {
    match s.parse::<u32>() {
        Ok(uid) => return Ok(Gid::from_raw(uid)),
        Err(_) => (),
    }

    return match Group::from_name(s) {
        Ok(g) => return Ok(g.unwrap().gid),
        Err(e) => Err(ParseError::Gid(e)),
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

pub(crate) fn parse_uid(s: &str) -> Result<Uid, ParseError> {
    match s.parse::<u32>() {
        Ok(uid) => return Ok(Uid::from_raw(uid)),
        Err(_) => (),
    }

    return match User::from_name(s) {
        Ok(u) => return Ok(u.unwrap().uid),
        Err(e) => Err(ParseError::Uid(e)),
    }
}

pub(crate) struct SystemdUnit {
    path: Option<PathBuf>,
    inner: Ini,
}

impl SystemdUnit {
    /// Appends `key=value` to last instance of `section`
    pub(crate) fn append_entry(&mut self, section: &str, key: &str, value: &str) {
        self.inner
            .with_section(Some(section))
            .append(key, value);
    }

    /// Retrun `true` if there's an (non-empty) instance of section `name`
    pub(crate) fn has_section(&self, name: &str) -> bool {
        match self.inner.section(Some(name)) {
            Some(_) => true,
            None => false,
        }
    }

    /// Number of unique sections (i.e. with different name)
    pub fn len(&self) -> usize {
        // rust-ini always includes the default/empty section
        self.inner.len() - 1
    }

    /// Load from a file
    pub fn load_from_file<P: AsRef<Path>>(filename: P) -> Result<Self, ini::Error> {
        Ok(SystemdUnit {
            path: Some(filename.as_ref().into()),
            inner: Ini::load_from_file(filename)?,
        })
    }

    /// Load from a string
    pub fn load_from_str(buf: &str) -> Result<Self, ini::ParseError> {
        Ok(SystemdUnit {
            path: None,
            inner: Ini::load_from_str_opt(
                buf,
                ParseOption {
                    //enabled_quote: false,
                    ..ParseOption::default()
                },
            )?,
        })
    }

    // Get an interator of values for all `key`s in all instances of `section`
    pub(crate) fn lookup_all<'a>(&'a self, section: &'a str, key: &'a str) -> impl DoubleEndedIterator<Item = &str> {
        self.inner.get_all_from(Some(section), key)
    }

    // Get the last value for `key` in all instances of `section`
    pub(crate) fn lookup_last<'a>(&'a self, section: &'a str, key: &'a str) -> Option<&'a str> {
        self.inner.get_last_from(Some(section), key)
    }

    pub(crate) fn new() -> Self {
        SystemdUnit {
            path: None,
            inner: Default::default(),
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

    pub(crate) fn rename_section(&mut self, from: &str, to: &str) {
        self.inner.rename_section_all(Some(from), Some(to));
    }

    pub(crate) fn section_entries<'a>(&'a self, name: &'a str) -> impl DoubleEndedIterator<Item=(&'a str, &'a str)> {
        self.inner
            .section_all(Some(name))
            .flat_map(|props| props.iter())
    }

    pub(crate) fn set_entry(&mut self, section: &str, key: &str, value: &str) {
        self.inner.set_to(Some(section), key.into(), value.into())
    }

    /// Write to a writer
    pub(crate) fn write_to<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        self.inner.write_to(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_gid {
        use nix::errno::Errno;

        use super::*;

        #[test]
        fn fails_with_empty_input() {
            let input = "";

            let res = parse_gid(input);
            assert_eq!(res.err(), Some(ParseError::Gid(Errno::ENOENT)));
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
            assert_eq!(res.err(), Some(ParseError::Gid(Errno::ENOENT)));
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
            assert_eq!(res.err(), Some(ParseError::Uid(Errno::ENOENT)));
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
            assert_eq!(res.err(), Some(ParseError::Uid(Errno::ENOENT)));
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
        #[ignore = "rust-ini (used internally) keeps an default/empty section"]
        fn test_key_without_section_should_fail() {
            let input = "KeyOne=value 1";

            let result = SystemdUnit::load_from_str(input);

            assert!(result.is_err());
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
            assert_eq!(iter.next().unwrap(), ("KeyTwo", "value 2"));
            assert_eq!(iter.next(), None);
        }

        #[test]
        #[ignore = "rust-ini wrongly trims values *after* unqoting"]
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
        #[ignore = "rust-ini doesn't handle inner quotes properly"]
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

        // TODO: automatically close quotes at end of line (with or witout line continuation)
        // TODO: test nested quotes
        // TODO: test all possible of escape sequences

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
            assert_eq!(iter.next().unwrap(), ("KeyOne", "value 1"));
            assert_eq!(iter.next().unwrap(), ("KeyTwo", "value 2"));
            assert_eq!(iter.next(), None);

            let mut iter = unit.section_entries("Section B");
            // TODO: may not be accurate according to Systemd quoting rules
            //assert_eq!(iter.next().unwrap(), ("Setting", "something some thing …"));
            assert_eq!(iter.next().unwrap(), ("Setting", "something \"some thing\" \"…\""));
            assert_eq!(iter.next().unwrap(), ("KeyTwo", "value 2        value 2 continued"));
            assert_eq!(iter.next(), None);

            let mut iter = unit.section_entries("Section C");
            assert_eq!(iter.next().unwrap(), ("KeyThree", "value 3       value 3 continued"));
            assert_eq!(iter.next(), None);
        }

        #[test]
        fn adapted_quadlet_case__escapes_container__succeeds() {
            let input = "[Container]
Image=imagename
PodmanArgs=\"--foo\" \\
  --bar
PodmanArgs=--also
Exec=/some/path \"an arg\" \"a;b\\nc\\td'e\" a;b\\nc\\td 'a\"b'";

            let unit = SystemdUnit::load_from_str(input).unwrap();
            assert_eq!(unit.len(), 1);

            let mut iter = unit.section_entries("Container");
            assert_eq!(iter.next().unwrap(), ("Image", "imagename"));
            assert_eq!(iter.next().unwrap(), ("PodmanArgs", "--foo    --bar"));
            assert_eq!(iter.next().unwrap(), ("PodmanArgs", "--also"));
            // TODO: may not be accurate according to Systemd quoting rules
            //assert_eq!(iter.next().unwrap(), ("Exec", "/some/path an arg a;b\nc\td'e a;b\nc\td a\"b"));
            assert_eq!(iter.next().unwrap(), ("Exec", "/some/path \"an arg\" \"a;b\nc\td'e\" a;b\nc\td 'a\"b'"));
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

}