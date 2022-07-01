use std::str::Chars;

const WHITESPACE: [char; 4] = [' ', '\t', '\n', '\r'];

pub struct SplitWord<'a> {
    //src: &'a str,  // the string to parse
    chars: Chars<'a>,  // `src.chars()`
    c: Option<char>,  // the current character
}

impl<'a> SplitWord<'a> {
    fn bump(&mut self) {
        self.c = self.chars.next();
    }

    fn eof(&self) -> bool {
        self.c.is_none()
    }

    pub fn new(src: &'a str) -> Self {
        let mut s = Self {
            //src: src,
            chars: src.chars(),
            c: None,
        };
        s.bump();
        s
    }

    pub fn next<'b>(&mut self) -> Option<String> {
        let mut word = String::new();

        // skip initial whitespace
        self.parse_until_none_of(&WHITESPACE);

        let mut quote: Option<char> = None;  // None or Some('\'') or Some('"')
        while let Some(c) = self.c {
            if let Some(q) = quote {
                // inside either single or double quotes
                word.push_str(self.parse_until_any_of(&[q]).as_str());

                match self.c {
                    Some(c) if c == q => {
                        quote = None
                    },
                    _ => (),
                }
            } else {
                match c {
                    '\'' | '"' => {
                        quote = Some(c)
                    },
                    _ if WHITESPACE.contains(&c) => {
                        // word is done
                        break
                    },
                    _ => word.push(c),
                }
            }

            self.bump();
        }

        if word.is_empty() {
            None
        } else {
            Some(word)
        }
    }

    fn parse_until_any_of(&mut self, end: &[char]) -> String {
        let mut s = String::new();

        while let Some(c) = self.c {
            if end.contains(&c) {
                break;
            }
            s.push(c);
            self.bump();
        }

        s
    }

    fn parse_until_none_of(&mut self, end: &[char]) -> String {
        let mut s = String::new();

        while let Some(c) = self.c {
            if !end.contains(&c) {
                break;
            }
            s.push(c);
            self.bump();
        }

        s
    }
}

impl<'a> Iterator for SplitWord<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        self.next()
    }
}

// impl<'a> IntoIterator for SplitWord<'a> {
//     type Item = &'a str;
//     type IntoIter = Self;

//     fn into_iter(self) -> Self::IntoIter {
//         self
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;

    mod split_word {
        use super::*;

        mod next {
            use super::*;

            #[test]
            fn none_with_empty_input() {
                let input = "";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), None);
            }

            #[test]
            fn none_with_only_whitespace() {
                let input = "\t    \r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), None);
            }

            #[test]
            fn some_with_simple_text() {
                let input = "\tfoo\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn some_with_multiple_words() {
                let input = "\tfoo bar\tbaz\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar".into()));
                assert_eq!(split.next(), Some("baz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn removes_quotes_arround_words() {
                let input = "\tfoo \"bar\"\t\'baz\'\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar".into()));
                assert_eq!(split.next(), Some("baz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn removes_quotes_inside_words() {
                let input = "\tfoo=\'bar\'  bar=\"baz\"\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo=bar".into()));
                assert_eq!(split.next(), Some("bar=baz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn keeps_spaces_inside_quotes() {
                let input = "\tfoo \"bar\tbaz\"\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar\tbaz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn keeps_nested_quotes() {
                let input = "\tfoo=\'bar \tbar=\"baz\"\'\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo=bar \tbar=\"baz\"".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn unmatched_quote_matches_till_end_of_line() {
                let input = "\tfoo=\'bar \tbar=\"baz\"\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo=bar \tbar=\"baz\"\r\n".into()));
                assert_eq!(split.next(), None);
            }
        }
    }
}