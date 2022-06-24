extern crate ini;

mod constants;

pub use self::constants::*;

use ini::Ini;
use std::{fmt, io};
use std::path::{PathBuf, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseBoolError;

impl fmt::Display for ParseBoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "value was neither `1`, `yes`, `true`, `on` nor `0`, `no`, `false`, `off`".fmt(f)
    }
}

pub(crate) fn parse_bool(s: &str) -> Result<bool, ParseBoolError> {
    if ["1", "yes", "true", "on"].contains(&s) {
        return Ok(true);
    } else if ["0", "no", "false", "off"].contains(&s) {
        return Ok(false)
    }

    Err(ParseBoolError)
}

pub(crate) struct SystemdUnit {
    path: Option<PathBuf>,
    inner: Ini,
}

impl SystemdUnit {
    pub(crate) fn add_entry(&mut self, section: &str, key: &str, value: &str) {
        // TODO: find out if this appends or replaces
        self.inner.set_to(Some(section), key.into(), value.into())
    }

    /// Load from a file
    pub fn load_from_file<P: AsRef<Path>>(filename: P) -> Result<Self, ini::Error> {
        Ok(SystemdUnit {
            path: Some(filename.as_ref().into()),
            inner: Ini::load_from_file(filename)?,
        })
    }

    pub(crate) fn lookup_all<'a>(&'a self, section: &'a str, key: &'a str) -> impl DoubleEndedIterator<Item = &str>
    {
        self.inner
            .section_all(Some(section))
            .flat_map(move |props| props.get_all(key))
    }

    pub(crate) fn lookup_last<'a>(&'a self, section: &'a str, key: &'a str) -> Option<&'a str> {
        self.lookup_all(section, key).last()
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
        let from_data: Vec<(&str, &str)> = self.inner
            .section_all(Some(from))
            .flat_map(|props| props.iter())
            .collect();

        for (k, v) in from_data {
            let mut to_section = self.inner.with_section(Some(to));
            to_section.set(k, v);
        }

        // TODO: find out if we have to loop until all `[from]` sections are gone
        self.inner.delete(Some(from));
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

    mod from_string {
        use super::*;

        #[test]
        fn test_parsing_ignores_comments() {
            let data = "#[Section A]
#KeyOne=value 1

;[Section B]
;KeyTwo=value 2";

            let unit = SystemdUnit::from_string(data).unwrap();

            assert_eq!(unit.sections.len(), 0);
        }

        #[test]
        fn test_simple_example() {
            let data = "[Section A]
KeyOne=value 1
KeyTwo=value 2";

            let unit = SystemdUnit::from_string(data).unwrap();

            assert_eq!(unit.sections.len(), 1);
            assert!(unit.section_entries("Section").is_empty());
            assert!(unit.section_entries("A").is_empty());
            assert_eq!(unit.section_entries("Section A").len(), 2);
        }

        #[test]
        fn test_with_same_section_occuring_multiple_times() {
            let data = "[Section A]
KeyOne=value 1
KeyTwo=value 2

[Section A]
KeyOne = value 1.2";

            let unit = SystemdUnit::from_string(data).unwrap();

            assert_eq!(unit.sections.len(), 2);
            assert_eq!(unit.section_entries("Section A").len(), 3);
        }

        #[test]
        fn test_key_without_section_should_fail() {
            let data = "KeyOne=value 1";

            let result = SystemdUnit::from_string(data);

            assert!(result.is_err());
        }

        #[test]
        fn test_systemd_syntax_example_1_succeeds() {
            let data = "[Section A]
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

            let unit = SystemdUnit::from_string(data).unwrap();

            assert_eq!(unit.sections.len(), 3);
            assert_eq!(
                unit,
                SystemdUnit {
                    path: None,
                    sections: vec![
                        Section {
                            name: "Section A".into(),
                            entries: vec![
                                ("KeyOne".into(), "value 1".into()),
                                ("KeyTwo".into(), "value 2".into()),
                            ],
                        },
                        Section {
                            name: "Section B".into(),
                            entries: vec![
                                ("Setting".into(), "\"something\" \"some thing\" \"…\"".into()),
                                ("KeyTwo".into(), "value 2        value 2 continued".into()),
                            ],
                        },
                        Section {
                            name: "Section C".into(),
                            entries: vec![
                                ("KeyThree".into(), "value 3       value 3 continued".into()),
                            ],
                        },
                    ],
                }
            );
        }


        #[test]
        fn test_quadlet_case_escapes_container_succeeds() {
            let data = "[Container]
Image=imagename
PodmanArgs=\"--foo\" \\
  --bar
PodmanArgs=--also
Exec=/some/path \"an arg\" \"a;b\\nc\\td'e\" a;b\\nc\\td 'a\"b'";

            let unit = SystemdUnit::from_string(data).unwrap();

            assert_eq!(unit.sections.len(), 1);
            assert_eq!(
                unit,
                SystemdUnit {
                    path: None,
                    sections: vec![
                        Section {
                            name: "Container".into(),
                            entries: vec![
                                ("Image".into(), "imagename".into()),
                                ("PodmanArgs".into(), "\"--foo\"    --bar".into()),
                                ("PodmanArgs".into(), "--also".into()),
                                ("Exec".into(), "/some/path \"an arg\" \"a;b\\nc\\td'e\" a;b\\nc\\td 'a\"b'".into()),
                            ],
                        },
                    ],
                }
            );
        }

    }
}