use std::str::Chars;

const WHITESPACE: [char; 4] = [' ', '\t', '\n', '\r'];

/// Splits a string at whitespace and removes quotes while preserving whitespace *inside* quotes.
/// It will *keep* escape sequences as they are (i.e. treat them as normal characters).
///
/// splits space separated values similar to the systemd config_parse_strv, merging multiple values into a single vector
/// equals behavior of Systemd's `extract_first_word()` with  `EXTRACT_RETAIN_ESCAPE|EXTRACT_UNQUOTE` flags
// EXTRACT_UNQUOTE       = Ignore separators in quoting with "" and '', and remove the quotes.
// EXTRACT_RETAIN_ESCAPE = Treat escape character '\' as any other character without special meaning
pub struct SplitStrv<'a> {
    chars: Chars<'a>,  // `src.chars()`
    c: Option<char>,  // the current character
}

impl<'a> SplitStrv<'a> {
    fn bump(&mut self) {
        self.c = self.chars.next();
    }

    pub fn new(src: &'a str) -> Self {
        let mut s = Self {
            chars: src.chars(),
            c: None,
        };
        s.bump();
        s
    }

    pub fn next<'b>(&mut self) -> Option<String> {
        let separators = &WHITESPACE;
        let mut word = String::new();

        // skip initial whitespace
        self.parse_until_none_of(separators);

        let mut quote: Option<char> = None;  // None or Some('\'') or Some('"')
        while let Some(c) = self.c {
            if let Some(q) = quote {
                // inside either single or double quotes
                match self.c {
                    Some(c) if c == q => {
                        quote = None
                    },
                    _ => word.push(c),
                }
            } else {
                match c {
                    '\'' | '"' => {
                        quote = Some(c)
                    },
                    _ if separators.contains(&c) => {
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

impl<'a> Iterator for SplitStrv<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        self.next()
    }
}

/// Splits a string at whitespace and removes quotes while preserving whitespace *inside* quotes.
/// It will also unescape known escape sequences.
///
/// equals behavior of Systemd's `extract_first_word()` with  `EXTRACT_RELAX|EXTRACT_UNQUOTE|EXTRACT_CUNESCAPE` flags
// EXTRACT_RELAX     = Allow unbalanced quote and eat up trailing backslash.
// EXTRACT_CUNESCAPE = Unescape known escape sequences.
// EXTRACT_UNQUOTE   = Ignore separators in quoting with "" and '', and remove the quotes.
pub struct SplitWord<'a> {
    chars: Chars<'a>,  // `src.chars()`
    c: Option<char>,  // the current character
}

impl<'a> SplitWord<'a> {
    fn bump(&mut self) {
        self.c = self.chars.next();
    }

    pub fn new(src: &'a str) -> Self {
        let mut s = Self {
            chars: src.chars(),
            c: None,
        };
        s.bump();
        s
    }

    pub fn next<'b>(&mut self) -> Option<String> {
        let separators = &WHITESPACE;
        let mut word = String::new();

        // skip initial whitespace
        self.parse_until_none_of(separators);

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
                    _ if separators.contains(&c) => {
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

    mod split_strv {
        use super::*;

        mod next {
            use super::*;

            #[test]
            fn none_with_empty_input() {
                let input = "";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), None);
            }

            #[test]
            fn none_with_only_whitespace() {
                let input = "\t    \r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), None);
            }

            #[test]
            fn some_with_simple_text() {
                let input = "\tfoo\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn some_with_multiple_words() {
                let input = "\tfoo bar\tbaz\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar".into()));
                assert_eq!(split.next(), Some("baz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn escaped_whitespace_is_part_of_word() {
                let input = "\tfoo bar\\tbaz\\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar\\tbaz\\r".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn removes_quotes_arround_words() {
                let input = "\tfoo \"bar\"\t\'baz\'\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar".into()));
                assert_eq!(split.next(), Some("baz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn removes_quotes_inside_words() {
                let input = "\tfoo \"bar\"\\t\'baz\'\\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar\\tbaz\\r".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn keeps_spaces_inside_quotes() {
                let input = "\tfoo \"bar\tbaz\"\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar\tbaz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn keeps_escape_sequences_inside_quotes() {
                let input = "\tfoo \"bar\\tbaz\"\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar\\tbaz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn keeps_nested_quotes() {
                let input = "\tfoo=\'bar \tbar=\"baz\"\'\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo=bar \tbar=\"baz\"".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn unmatched_quote_matches_till_end_of_line() {
                let input = "\tfoo=\'bar \tbar=\"baz\"\r\n";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo=bar \tbar=\"baz\"\r\n".into()));
                assert_eq!(split.next(), None);
            }
        }
    }

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