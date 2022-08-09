use std::str::Chars;

pub fn unquote_value(raw: &str) -> Result<String, String> {
    let mut parser = Quoted {
        chars: raw.chars(),
        cur: None,
    };
    parser.bump();

    parser.parse_and_unquote()
}

struct Quoted<'a> {
    chars: Chars<'a>,
    cur: Option<char>,
}

impl<'a> Quoted<'a> {
    fn bump(&mut self) {
        self.cur = self.chars.next();
    }

    fn parse_and_unquote(&mut self) -> Result<String, String> {
        let mut result: String = String::new();
        let mut quote: Option<char> = None;

        while self.cur.is_some() {
            match self.cur {
                None => return Err("found early EOF".into()),
                Some('\'' | '"') if result.ends_with([' ', '\t', '\n']) || result.is_empty() => {
                    quote = self.cur;
                }
                Some('\\') => {
                    self.bump();
                    match self.cur {
                        None => return Err("expecting escape sequence, but found EOF.".into()),
                        // line continuation (i.e. value continues on the next line)
                        Some(_) => result.push(self.parse_escape_sequence()?),
                    }
                }
                Some(c) => {
                    if self.cur == quote {
                        // inside either single or double quotes
                        quote = None;
                    } else {
                        result.push(c);
                    }
                }
            }
            self.bump();
        }
        Ok(result)
    }

    fn parse_escape_sequence(&mut self) -> Result<char, String> {
        if let Some(c) = self.cur {
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
                c => return Err(format!("expecting escape sequence, but found {c:?}."))
            };

            Ok(r)
        } else {
            return Err("expecting escape sequence, but found EOF.".into())
        }
    }

    fn parse_unicode_escape(&mut self, prefix: Option<char>, max_chars: usize, radix: u32) -> Result<char, String> {
        assert!(prefix.is_none() || (prefix.is_some() && ['x', 'u', 'U'].contains(&prefix.unwrap())));
        assert!([8, 16].contains(&radix));

        let mut code = String::with_capacity(max_chars);
        for _ in 0..max_chars {
            if let Some(c) = self.cur {
                code.push(c);
                if radix == 16 && !c.is_ascii_hexdigit() {
                    return Err(format!("Expected {max_chars} hex values after \"\\{c}\", but got \"\\{c}{code}\"" ))
                } else if radix == 8 && (!c.is_ascii_digit() || c == '8' || c == '9') {
                    return Err(format!("Expected {max_chars} octal values after \"\\\", but got \"\\{code}\"" ))
                }
            } else {
                return Err("expecting unicode escape sequence, but found EOF.".into())
            }

            if code.len() != max_chars {
                self.bump();
            }
        }

        let ucp = u32::from_str_radix(code.as_str(), radix).unwrap();
        if ucp == 0 {
            return Err("\\0 character not allowed in escape sequence".into())
        }

        return match char::try_from(ucp) {
            Ok(u) => Ok(u),
            Err(e) => Err(format!("invalid unicode character in escape sequence: {e}")),
        }
    }

    fn parse_until_any_of(&mut self, end: &[char]) -> String {
        let mut s = String::new();

        while let Some(c) = self.cur {
            if end.contains(&c) {
                break;
            }
            s.push(c);
            self.bump();
        }

        s
    }
}

mod tests {
    mod unquote_value {
        use super::super::unquote_value;

        #[test]
        fn keeps_quotes_inside_words() {
            let input = "foo=\'bar\' \"bar=baz\"";

            assert_eq!(
                unquote_value(input),
                Ok("foo=\'bar\' bar=baz".into()),
            );
        }

        #[test]
        fn keeps_spaces_inside_quotes() {
            let input = "foo \"bar\tbaz\"";

            assert_eq!(
                unquote_value(input),
                Ok("foo bar\tbaz".into()),
            );
        }

        #[test]
        fn keeps_nested_quotes() {
            let input = "\'bar \tbar=\"baz\"\'";

            assert_eq!(
                unquote_value(input),
                Ok("bar \tbar=\"baz\"".into()),
            );
        }

        #[test]
        fn unescapes_single_character_sequences() {
            let input = "\\a\\b\\f\\n\\r\\t\\v\\\\\\\"\\\'\\s";

            assert_eq!(
                unquote_value(input),
                Ok("\u{7}\u{8}\u{c}\n\r\t\u{b}\\\"\' ".into()),
            );
        }

        #[test]
        fn unescapes_unicode_sequences() {
            let input = "\\xaa \\u1234 \\U0010cdef \\123";

            assert_eq!(
                unquote_value(input),
                Ok("\u{aa} \u{1234} \u{10cdef} \u{53}".into()),
            );
        }

        #[test]
        fn fails_with_escaped_null() {
            let input = "\\x00";

            assert_eq!(
                unquote_value(input),
                Err("\\0 character not allowed in escape sequence".into()),
            );
        }

        #[test]
        fn fails_with_illegal_digit() {
            let input = "\\u123x";

            assert_eq!(
                unquote_value(input),
                Err("Expected 4 hex values after \"\\x\", but got \"\\x123x\"".into()),
            );
        }

        #[test]
        fn fails_with_illegal_octal_digit() {
            let input = "\\678";

            assert_eq!(
                unquote_value(input),
                Err("Expected 3 octal values after \"\\\", but got \"\\678\"".into()),
            );
        }

        #[test]
        fn fails_with_incomplete_sequence() {
            let input = "\\";

            assert_eq!(
                unquote_value(input),
                Err("expecting escape sequence, but found EOF.".into()),
            );
        }

        #[test]
        fn fails_with_incomplete_unicode_sequence() {
            let input = "\\u12";

            assert_eq!(
                unquote_value(input),
                Err("expecting unicode escape sequence, but found EOF.".into()),
            );
        }

        #[test]
        fn fails_with_unknown_escape_char() {
            let input = "\\_";

            assert_eq!(
                unquote_value(input),
                Err("expecting escape sequence, but found '_'.".into()),
            );
        }
    }
}