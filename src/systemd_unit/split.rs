use std::char::CharTryFromError;
use std::fmt::Display;
use std::str::Chars;

const WHITESPACE: [char; 4] = [' ', '\t', '\n', '\r'];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EscapeError {
    InvalidDigit(Option<char>, String),
    NullNotAllowed,
    SequenceIncomplete,
    UnicodeCharInvalid(CharTryFromError),
    UnknownSequence(char),
}

impl Display for EscapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EscapeError::InvalidDigit(c, code) => {
                let max_chars = match c {
                    Some('x') => 2,
                    Some('u') => 4,
                    Some('U') => 8,
                    None => 3,
                    _ => unreachable!(),
                };
                let val = match c {
                    None => "octal",
                    _ => "hex",
                };
                let c_s = match c {
                    None => String::new(),
                    Some(c) => c.to_string(),
                };
                write!(f, "Expected {max_chars} {val} values after \"\\{c_s}\", but got \"{code}\"" )
            },
            EscapeError::NullNotAllowed => {
                write!(f, "Escape sequence for '\\0' is not allowed")
            },
            EscapeError::SequenceIncomplete => {
                write!(f, "Incomplete escape sequence")
            },
            EscapeError::UnicodeCharInvalid(e) => {
                write!(f, "Failed to convert escape sequence to Unicode character: {e}")
            },
            EscapeError::UnknownSequence(c) => {
                write!(f, "Unknown escape sequence \"\\{c}\"")
            },
        }
    }
}

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
        let mut backslash = false;  // whether we've just seen a backslash
        while let Some(c) = self.c {
            if backslash {
                // first character after '\'

                match self.parse_escape_sequence() {
                    Ok(e) => word.push(e),
                    Err(_) => return None,
                }

                backslash = false;
            } else if let Some(q) = quote {
                // inside either single or double quotes
                word.push_str(self.parse_until_any_of(&[q, '\\']).as_str());

                match self.c {
                    Some(c) if c == q => {
                        quote = None
                    },
                    Some('\\') => {
                        backslash = true
                    }
                    _ => (),
                }
            } else {
                match c {
                    '\'' | '"' => {
                        quote = Some(c)
                    },
                    '\\' => {
                        backslash = true
                    }
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

    fn parse_escape_sequence(&mut self) -> Result<char, EscapeError> {
        if let Some(c) = self.c {
            let r = match c {
                'a'  => '\u{7}',
                'b'  => '\u{8}',
                'f'  => '\u{c}',
                'n'  => '\n',
                'r'  => '\r',
                't'  => '\t',
                'v'  => '\u{b}',
                '\\' => '\\',
                '"'  => '"',
                '\'' => '\'',
                's'  => ' ',

                'x'  => {  // 2 character hex encoding
                    self.bump();
                    self.parse_unicode_escape(Some('x'), 2, 16)?
                },
                'u'  => {  // 4 character hex encoding
                    self.bump();
                    self.parse_unicode_escape(Some('u'), 4, 16)?
                },
                'U'  => {  // 8 character hex encoding
                    self.bump();
                    self.parse_unicode_escape(Some('U'), 8, 16)?
                },
                '0'..='7' => {  // 3 character octal encoding
                    self.parse_unicode_escape(None, 3, 8)?
                }
                _ => {
                    return Err(EscapeError::UnknownSequence(c))
                }
            };

            Ok(r)
        } else {
            return Err(EscapeError::SequenceIncomplete)
        }
    }

    fn parse_unicode_escape(&mut self, prefix: Option<char>, max_chars: usize, radix: u32) -> Result<char, EscapeError> {
        assert!(prefix.is_none() || (prefix.is_some() && ['x', 'u', 'U'].contains(&prefix.unwrap())));
        assert!([8, 16].contains(&radix));

        let mut code = String::with_capacity(max_chars);
        for _ in 0..max_chars {
            if let Some(c) = self.c {
                code.push(c);
                if radix == 16 && !c.is_ascii_hexdigit() {
                    return Err(EscapeError::InvalidDigit(prefix, code))
                } else if radix == 8 && (!c.is_ascii_digit() || c == '8' || c == '9') {
                    return Err(EscapeError::InvalidDigit(prefix, code))
                }
            } else {
                return Err(EscapeError::SequenceIncomplete)
            }

            if code.len() != max_chars {
                self.bump();
            }
        }

        let ucp = u32::from_str_radix(code.as_str(), radix).unwrap();
        if ucp == 0 {
            return Err(EscapeError::NullNotAllowed)
        }

        return match char::try_from(ucp) {
            Ok(u) => Ok(u),
            Err(e) => Err(EscapeError::UnicodeCharInvalid(e)),
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

            #[test]
            fn escapes() {
                let input = "\\a\\b\\f\\n\\r\\t\\v\\\\\\\"\\\'\\s";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("\u{7}\u{8}\u{c}\n\r\t\u{b}\\\"\' ".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn escapes_unicode_sequences() {
                let input = "xx\\xaaxx uu\\u1234uu  UU\\U0010cdefUU oo\\123oo";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("xx\u{aa}xx".into()));
                assert_eq!(split.next(), Some("uu\u{1234}uu".into()));
                assert_eq!(split.next(), Some("UU\u{10cdef}UU".into()));
                assert_eq!(split.next(), Some("oo\u{53}oo".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn escapes_in_quotes() {
                let input = "\tfoo=\'bar\\t\\'baz\\'\'\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo=bar\t\'baz\'".into()));
                assert_eq!(split.next(), None);
            }
        }

        mod parse_escape_sequence {
            use super::*;

            #[test]
            fn fails_with_unknown_escape_char() {
                let input = "_";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Err(EscapeError::UnknownSequence('_')));
                split.bump();
                assert!(split.eof());
            }

            #[test]
            fn unescapes_single_character_sequences() {
                let input = "abfnrtv\\\"\'s";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Ok('\u{7}'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\u{8}'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\u{c}'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\n'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\r'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\t'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\u{b}'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\\'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\"'));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\''));
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok(' '));
                split.bump();
                assert!(split.eof());
            }

            #[test]
            fn unescapes_unicode_sequences() {
                let input = "xaa u1234 U0010cdef 123";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Ok('\u{aa}'));
                split.bump();
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\u{1234}'));
                split.bump();
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\u{10cdef}'));
                split.bump();
                split.bump();
                assert_eq!(split.parse_escape_sequence(), Ok('\u{53}'));
                split.bump();
                assert!(split.eof());
            }

            #[test]
            fn fails_with_escaped_null() {
                let input = "x00";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Err(EscapeError::NullNotAllowed));
                split.bump();
                assert!(split.eof());
            }

            #[test]
            fn fails_with_illegal_digit() {
                let input = "u123x";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Err(EscapeError::InvalidDigit(Some('u'), "123x".into())));
                split.bump();
                assert!(split.eof());
            }

            #[test]
            fn fails_with_illegal_octal_digit() {
                let input = "678";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Err(EscapeError::InvalidDigit(None, "678".into())));
                split.bump();
                assert!(split.eof());
            }

            #[test]
            fn fails_with_incomplete_sequence() {
                let input = "";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Err(EscapeError::SequenceIncomplete));
                split.bump();
                assert!(split.eof());
            }

            #[test]
            fn fails_with_incomplete_unicode_sequence() {
                let input = "u12";

                let mut split = SplitWord::new(input);
                assert_eq!(split.parse_escape_sequence(), Err(EscapeError::SequenceIncomplete));
                split.bump();
                assert!(split.eof());
            }
        }
    }
}