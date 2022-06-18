use std::collections::HashMap;

struct SystemdUnit {
  entries_by_section: HashMap<String, HashMap<String, String>>,
}

impl SystemdUnit {
    fn from_string(data: &str) -> Result<Self, parser::ParseError> {
      todo!()
    }

    fn sections(&self) -> Vec<String> {
        // self.entries_by_section.keys().collect();
        todo!()
    }

    fn section_entries(&self, section: &str) -> Option<&HashMap<String, String>> {
        self.entries_by_section.get(section)
    }
}

#[cfg(test)]
mod test {
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

            assert_eq!(unit.sections().len(), 0);
        }

        #[test]
        fn test_simple_example() {
            let data = "[Section A]
    KeyOne=value 1
    KeyTwo=value 2";

            let unit = SystemdUnit::from_string(data).unwrap();

            assert_eq!(unit.sections().len(), 1);
            assert!(unit.section_entries("Section").is_none());
            assert!(unit.section_entries("A").is_none());
            assert!(unit.section_entries("Section A").is_some());
            assert_eq!(unit.section_entries("Section A").unwrap().len(), 2);
        }

        #[test]
        fn test_key_without_section_should_fail() {
            let data = "KeyOne=value 1";

            let result = SystemdUnit::from_string(data);

            assert!(result.is_err());
        }

        #[test]
        fn test_spaces_around_equals_should_be_ignored() {
            todo!()
        }

        #[test]
        fn test_multi_line_values_should_work() {
            todo!()
        }

        #[test]
        fn test_multi_line_values_with_comments_in_between_should_work() {
            todo!()
        }
    }
}