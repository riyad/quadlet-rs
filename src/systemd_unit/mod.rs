mod constants;
mod parser;

pub use self::constants::*;

use std::io;
use std::path::PathBuf;

type Error = parser::ParseError;

#[derive(Debug, PartialEq)]
pub(crate) struct SystemdUnit {
    pub(crate) path: Option<PathBuf>,
    pub(crate) sections: Vec<Section>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Section {
    name: String,
    entries: Vec<Entry>,
}

impl Section {
    pub(crate) fn new(name: String) -> Self {
        // FIXME: validate name
        let mut ret = Self::_new();
        ret.name = name;
        ret
    }

    fn _new() -> Self {
        Section {
            name: Default::default(),
            entries: Default::default(),
        }
    }
}

type Entry = (Key, Value);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Key(String);

impl From<&str> for Key {
    fn from(key: &str) -> Self {
        // FIXME: validate str
        Self(key.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Value(String);

impl From<&str> for Value {
    fn from(val: &str) -> Self {
        Self(val.to_owned())
    }
}

impl ToString for Value {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

impl Value {
    fn to_quoted(&self) -> Vec<&str> {
        todo!()
    }
}

impl SystemdUnit {
    pub(crate) fn add_entry(&mut self, group_name: &str, key: &str, value: &str) {
        let entry = (key.into(), value.into());
        match self.sections.iter_mut().find(|s| s.name == group_name) {
            Some(section) => section.entries.push(entry),
            None => {
                self.sections.push(Section {
                    name: group_name.to_owned(),
                    entries: vec![entry],
                });
            },
        };
    }

    pub(crate) fn from_string(data: &str) -> Result<Self, Error> {
        let tokens = parser::lexer::Lexer::tokens_from(data)?;
        let mut parser = parser::Parser::new(tokens);
        let unit = parser.parse()?;

        Ok(unit)
    }

    pub(crate)fn lookup_all(&self, lookup_section: &str, lookup_key: &str) -> Vec<&Value> {
        self.sections
            .iter()
            .filter(|s| s.name == lookup_section)
            .map(|s| &s.entries)
            .flatten()
            .filter(|(k, _v)| k.0 == lookup_key )
            .map(|(_k,v)| v)
            .collect()
    }

    pub(crate) fn lookup_last(&self, lookup_section: &str, lookup_key: &str) -> Option<&Value> {
        self.sections
            .iter()
            .filter(|s| s.name == lookup_section)
            .map(|s| &s.entries)
            .flatten()
            .find(|(k, _v)| k.0 == lookup_key )
            .map(|(_k,v)| v)
    }

    pub(crate) fn new() -> Self {
        SystemdUnit {
            path: None,
            sections: Vec::default()
        }
    }

    pub(crate) fn merge_from(&mut self, other: &SystemdUnit) {
        for other_section in &other.sections {
            self.sections.push(other_section.clone());
        }
    }

    pub(crate) fn rename_section(&mut self, from: &str, to: &str) {
        let _ = self.sections
            .iter_mut()
            .filter(|s| s.name == from)
            .map(|s| s.name = to.to_owned());
    }

    fn section_entries(&self, section_name: &str) -> Vec<Entry> {
        self.sections.iter()
            .filter(|s| s.name == section_name)
            .map(|s| s.entries.clone())
            .flatten()
            .collect()
    }

    pub(crate) fn section_names(&self) -> Vec<String> {
        // FIXME: make sure list only has unique elements
        self.sections.iter().map(|s| s.name.clone()).collect()
    }

    pub(crate) fn set_entry(&mut self, section_name: &str, key: &str, value: &str) {
        let section = match self.sections
                .iter_mut()
                .find(|s| s.name == section_name)
                .map(|s| s) {
            Some(s) => s,
            None =>  {
                let s = Section::new(section_name.to_owned());
                self.sections.push(s);
                self.sections.iter_mut().last().unwrap()
            },
        };

        let entry = (key.into(), value.into());

        if section.entries.len() == 0 {
            section.entries.push(entry);
        } else {
            // find index of last occurrence of key
            let (i, _) = section.entries
                .iter_mut()
                .enumerate()
                .rev()
                .find(|(_i, (k,_v))| k.0 == key).unwrap();
            // replace that entry
            section.entries.insert(i, entry);
        }
    }

    pub fn write_to<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        for section in &self.sections {
            write!(writer, "[{}]\n", section.name)?;
            for (k, v) in &section.entries {
                write!(writer, "{k:?}={v:?}\n")?;
            }
        }

        Ok(())
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