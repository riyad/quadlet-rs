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
    chars: Chars<'a>, // `src.chars()`
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

    pub fn next(&mut self) -> Option<String> {
        let separators = &WHITESPACE;
        let mut word = String::new();

        // skip initial whitespace
        self.parse_until_none_of(separators);

        let mut quote: Option<char> = None; // None or Some('\'') or Some('"')
        while let Some(c) = self.c {
            if let Some(q) = quote {
                // inside either single or double quotes
                match self.c {
                    Some(c) if c == q => quote = None,
                    _ => word.push(c),
                }
            } else {
                match c {
                    '\'' | '"' => quote = Some(c),
                    _ if separators.contains(&c) => {
                        // word is done
                        break;
                    }
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

impl Iterator for SplitStrv<'_> {
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
    chars: Chars<'a>, // `src.chars()`
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

    pub fn next(&mut self) -> Option<String> {
        let separators = &WHITESPACE;
        let mut word = String::new();

        // skip initial whitespace
        self.parse_until_none_of(separators);

        let mut quote: Option<char> = None; // None or Some('\'') or Some('"')
        let mut backslash = false; // whether we've just seen a backslash
        while let Some(c) = self.c {
            if backslash {
                match self.parse_escape_sequence() {
                    Ok(r) => word.push(r),
                    Err(_) => return None,
                };

                backslash = false;
            } else if let Some(q) = quote {
                // inside either single or double quotes
                word.push_str(self.parse_until_any_of(&[q, '\\']).as_str());

                match self.c {
                    Some(c) if c == q => {
                        quote = None;
                    }
                    Some('\\') => backslash = true,
                    _ => (),
                }
            } else {
                match c {
                    '\'' | '"' => quote = Some(c),
                    '\\' => {
                        backslash = true;
                    }
                    _ if separators.contains(&c) => {
                        // word is done
                        break;
                    }
                    _ => word.push(c),
                }
            }

            self.bump();
        }

        // if backslash {
        //     // do nothing -> eat up trailing backslash
        //     // otherwise we'd have to push it onto `word`
        // }

        if word.is_empty() {
            None
        } else {
            Some(word)
        }
    }

    fn parse_escape_sequence(&mut self) -> Result<char, String> {
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

                'x' => {
                    // 2 character hex encoding
                    self.bump();
                    self.parse_unicode_escape(Some('x'), 2, 16)?
                }
                'u' => {
                    // 4 character hex encoding
                    self.bump();
                    self.parse_unicode_escape(Some('u'), 4, 16)?
                }
                'U' => {
                    // 8 character hex encoding
                    self.bump();
                    self.parse_unicode_escape(Some('U'), 8, 16)?
                }
                '0'..='7' => {
                    // 3 character octal encoding
                    self.parse_unicode_escape(None, 3, 8)?
                }
                c => c,
            };

            Ok(r)
        } else {
            Err("expecting escape sequence, but found EOF.".into())
        }
    }

    fn parse_unicode_escape(
        &mut self,
        prefix: Option<char>,
        max_chars: usize,
        radix: u32,
    ) -> Result<char, String> {
        assert!(
            prefix.is_none() || (prefix.is_some() && ['x', 'u', 'U'].contains(&prefix.unwrap()))
        );
        assert!([8, 16].contains(&radix));

        let mut code = String::with_capacity(max_chars);
        for _ in 0..max_chars {
            if let Some(c) = self.c {
                code.push(c);
                if radix == 16 && !c.is_ascii_hexdigit() {
                    return Err(format!(
                        "Expected {max_chars} hex values after \"\\{c}\", but got \"\\{c}{code}\""
                    ));
                } else if radix == 8 && (!c.is_ascii_digit() || c == '8' || c == '9') {
                    return Err(format!(
                        "Expected {max_chars} octal values after \"\\\", but got \"\\{code}\""
                    ));
                }
            } else {
                return Err("expecting unicode escape sequence, but found EOF.".into());
            }

            if code.len() != max_chars {
                self.bump();
            }
        }

        let ucp = u32::from_str_radix(code.as_str(), radix).unwrap();
        if ucp == 0 {
            return Err("\\0 character not allowed in escape sequence".into());
        }

        match char::try_from(ucp) {
            Ok(u) => Ok(u),
            Err(e) => Err(format!("invalid unicode character in escape sequence: {e}")),
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

impl<'a> Default for SplitWord<'_> {
    fn default() -> Self {
        Self {
            chars: "".chars(),
            c: Default::default(),
        }
    }
}

impl<'a> Iterator for SplitWord<'_> {
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
            #[ignore = "doesn't seem to work yet"]
            fn some_with_empty_word() {
                let input = "\tfoo \"\"";

                let mut split = SplitStrv::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("".into()));
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
            #[ignore = "doesn't seem to work yet"]
            fn some_with_empty_word() {
                let input = "\tfoo \"\"";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("".into()));
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
                let input = "\tfoo=\'bar \tbar=\"baz\"";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo=bar \tbar=\"baz\"".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn unescapes_escape_sequences() {
                let input = "\\tfoo\\u1234";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("\tfoo\u{1234}".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn keeps_escaped_whitespace() {
                let input = "\\tfoo bar\\tbaz\\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("\tfoo".into()));
                assert_eq!(split.next(), Some("bar\tbaz\n".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn unescapes_spaces_inside_quotes() {
                let input = "\tfoo \"bar\\tbaz\"\r\n";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar\tbaz".into()));
                assert_eq!(split.next(), None);
            }

            #[test]
            fn eats_up_trailing_backslash() {
                let input = "\tfoo bar\\";

                let mut split = SplitWord::new(input);
                assert_eq!(split.next(), Some("foo".into()));
                assert_eq!(split.next(), Some("bar".into()));
                assert_eq!(split.next(), None);
            }
        }
    }
}
