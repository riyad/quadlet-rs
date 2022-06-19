mod parser;

type Error = parser::ParseError;

#[derive(Debug, PartialEq)]
pub(crate) struct SystemdUnit {
    pub(crate) sections: Vec<Section>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Section {
    name: String,
    entries: Vec<Entry>,
}
type Entry = (Key, Value);
type Key = String;
type Value = String;

impl SystemdUnit {
    fn from_string(data: &str) -> Result<Self, Error> {
        let tokens = parser::lexer::Lexer::tokens_from(data)?;
        let mut parser = parser::Parser::new(tokens);
        let unit = parser.parse()?;

        Ok(unit)
    }

    fn new() -> Self {
        SystemdUnit { sections: Vec::default() }
    }

    fn section_names(&self) -> Vec<String> {
        self.sections.iter().map(|s| s.name.clone()).collect()
    }

    fn section_entries(&self, section_name: &str) -> Vec<Entry> {
        self.sections.iter()
            .filter(|s| s.name == section_name)
            .map(|s| s.entries.clone())
            .flatten()
            .collect()
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