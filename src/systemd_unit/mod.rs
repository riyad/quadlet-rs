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

    fn section_entries(&self, section_name: &str) -> Vec<&Section> {
        self.sections.iter().filter(|&s| s.name == section_name).collect()
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
        fn test_key_without_section_should_fail() {
            let data = "KeyOne=value 1";

            let result = SystemdUnit::from_string(data);

            assert!(result.is_err());
        }

        #[test]
        fn test_spaces_around_equals_should_be_ignored() {
        }

        #[test]
        fn test_multi_line_values_should_work() {
        }

        #[test]
        fn test_multi_line_values_with_comments_in_between_should_work() {
        }
    }
}