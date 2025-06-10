use super::*;

use std::str::Chars;

const LINE_CONTINUATION_REPLACEMENT: &str = " ";

type ParseResult<T> = Result<T, ParseError>;
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
#[error("{line}:{col} {msg}")]
pub struct ParseError {
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) msg: String,
}

#[derive(Debug)]
pub(crate) struct Parser<'a> {
    cur: Option<char>,
    buf: Chars<'a>,
    line: usize,
    column: usize,
}

impl<'a> Parser<'a> {
    pub fn new(buf: &'a str) -> Self {
        let mut p = Self {
            cur: None,
            buf: buf.chars(),
            line: 0,
            column: 0,
        };
        p.bump();
        p
    }

    fn bump(&mut self) {
        self.cur = self.buf.next();
        match self.cur {
            Some('\n') => {
                self.line += 1;
                self.column = 0;
            }
            Some(..) => {
                self.column += 1;
            }
            None => {}
        }
    }

    #[cold]
    fn error(&self, msg: String) -> ParseError {
        ParseError {
            line: self.line,
            col: self.column,
            msg,
        }
    }

    pub(crate) fn parse(&mut self) -> ParseResult<SystemdUnitData> {
        self.parse_unit()
    }

    // COMMENT        = ('#' | ';') ANY* NL
    fn parse_comment(&mut self) -> ParseResult<String> {
        match self.cur {
            Some('#' | ';') => (),
            Some(c) => return Err(self.error(format!("expected comment, but found {c:?}"))),
            None => return Err(self.error("expected comment, but found EOF".into())),
        }

        let comment = self.parse_until_any_of(&['\n']);
        Ok(comment)
    }

    // ENTRY          = KEY WS* '=' WS* VALUE NL
    fn parse_entry(&mut self) -> ParseResult<(EntryKey, EntryRawValue)> {
        let key = self.parse_key()?;

        // skip whitespace before '='
        self.skip_chars(&[' ', '\t']);
        match self.cur {
            Some('=') => self.bump(),
            Some(c) => return Err(self.error(format!("expected '=' after key, but found {c:?}"))),
            None => return Err(self.error("expected '=' after key, but found EOF".into())),
        }
        // skip whitespace after '='
        self.skip_chars(&[' ', '\t']);

        let value = self.parse_value()?;

        Ok((key, value))
    }

    // KEY            = [A-Za-z0-9-]
    fn parse_key(&mut self) -> ParseResult<EntryKey> {
        let key: String =
            self.parse_until_any_of(&['=', /*+ WHITESAPCE*/ ' ', '\t', '\n', '\r']);

        if !key.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return Err(self.error(format!(
                "Invalid key {:?}. Allowed characters are A-Za-z0-9-",
                key
            )));
        }

        Ok(key)
    }

    // SECTION        = SECTION_HEADER [COMMENT | ENTRY]*
    fn parse_section(&mut self) -> ParseResult<(SectionKey, Vec<(EntryKey, EntryRawValue)>)> {
        let name = self.parse_section_header()?;
        let mut entries: Vec<(EntryKey, EntryRawValue)> = Vec::new();

        while let Some(c) = self.cur {
            match c {
                '#' | ';' => {
                    // ignore comment
                    let _ = self.parse_comment();
                }
                '[' => break,
                _ if c.is_ascii_whitespace() => self.bump(),
                _ => {
                    entries.push(self.parse_entry()?);
                }
            }
        }

        Ok((name, entries))
    }

    // SECTION_HEADER = '[' ANY+ ']' NL
    fn parse_section_header(&mut self) -> ParseResult<String> {
        match self.cur {
            Some('[') => self.bump(),
            Some(c) => {
                return Err(self.error(format!(
                    "expected '[' as start of section header, but found {c:?}"
                )))
            }
            None => {
                return Err(
                    self.error("expected '[' as start of section header, but found EOF".into())
                )
            }
        }

        let section_name = self.parse_until_any_of(&[']', '\n']);

        match self.cur {
            Some(']') => self.bump(),
            Some(c) => {
                return Err(self.error(format!(
                    "expected ']' as end of section header, but found {c:?}"
                )))
            }
            None => {
                return Err(
                    self.error("expected ']' as end of section header, but found EOF".into())
                )
            }
        }

        if section_name.is_empty() {
            return Err(self.error("section header cannot be empty".into()));
        } else {
            // TODO: validate section name
        }

        // TODO: silently accept whitespace until EOL

        Ok(section_name)
    }

    // UNIT           = [COMMENT | SECTION]*
    fn parse_unit(&mut self) -> ParseResult<SystemdUnitData> {
        let mut unit = SystemdUnitData::new();

        while let Some(c) = self.cur {
            match c {
                '#' | ';' => {
                    // ignore comment
                    let _ = self.parse_comment();
                }
                '[' => {
                    let (section, entries) = self.parse_section()?;
                    // make sure there's a section entry (even if `entries` is empty)
                    unit.sections
                        .entry(section.clone())
                        .or_insert(Entries::default());
                    for (key, value) in entries {
                        unit.add_raw(section.as_str(), key, value.as_str())
                            .map_err(|e| self.error(e.to_string()))?;
                    }
                }
                _ if c.is_ascii_whitespace() => self.bump(),
                _ => return Err(self.error("Expected comment or section".into())),
            };
        }

        Ok(unit)
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

    fn skip_chars(&mut self, end: &[char]) {
        while let Some(c) = self.cur {
            if !end.contains(&c) {
                break;
            }
            self.bump();
        }
    }

    // VALUE          = ANY* CONTINUE_NL [COMMENT]* VALUE
    fn parse_value(&mut self) -> ParseResult<EntryRawValue> {
        let mut value: String = String::new();
        let mut backslash = false;
        let mut line_continuation = false;
        let mut line_continuation_ignored_spaces = 0;

        while let Some(c) = self.cur {
            if backslash {
                backslash = false;
                match c {
                    // for leniency we ignore spaces between the '\' and the '\n' of a line continuation
                    ' ' => {
                        line_continuation_ignored_spaces += 1;
                        backslash = true; // pretend this is still the case :/
                    }
                    // line continuation -> add replacement to value and continue normally
                    '\n' => {
                        value.push_str(LINE_CONTINUATION_REPLACEMENT);
                        line_continuation = true;
                    }
                    // just an escape sequence -> add to value and continue normally
                    _ => {
                        value.push('\\');
                        // restore ignored spaces (see above)
                        for _ in 0..line_continuation_ignored_spaces {
                            value.push(' ')
                        }
                        value.push(c);
                    }
                }
            } else if line_continuation {
                line_continuation = false;
                line_continuation_ignored_spaces = 0;
                match c {
                    '#' | ';' => {
                        // ignore interspersed comments
                        let _ = self.parse_comment();
                        line_continuation = true;
                    }
                    // end of value
                    '\n' => break,
                    // start of section header (although an unexpected one), i.e. end of value
                    // NOTE: we're trying to be clever here and assume the line continuation was a mistake
                    '[' => break,
                    // value continues after line continuation, add the actual line
                    // continuation characters back to value and continue normally
                    _ => {
                        if c == '\\' {
                            // we may have a line continuation following another line continuation
                            backslash = true;
                        } else {
                            value.push(c);
                        }
                    }
                }
            } else {
                match c {
                    // may be start of a line continuation
                    '\\' => backslash = true,
                    // end of value
                    '\n' => break,
                    _ => value.push(c),
                }
            }
            self.bump();
        }

        Ok(value.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_comment {
        use super::*;

        #[test]
        fn test_success_consumes_token_hash() {
            let input = "# foo\n; bar";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let _old_col = parser.column;
            assert_eq!(parser.parse_comment(), Ok("# foo".into()));
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, 0);
        }

        #[test]
        fn test_success_consumes_token_semicolon() {
            let input = "; foo\n# bar";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let _old_col = parser.column;
            assert_eq!(parser.parse_comment(), Ok("; foo".into()));
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, 0);
        }

        #[test]
        #[ignore = "until proper comment handling is implemented"]
        fn test_combining_multiple_lines() {
            let input = "# foo\n; bar";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let _old_col = parser.column;
            assert_eq!(parser.parse_comment(), Ok("# foo\n; bar".into()));
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, 0);
        }

        #[test]
        fn test_ignore_line_coninuation() {
            let input = "# foo \\\nbar=baz";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let _old_col = parser.column;
            assert_eq!(parser.parse_comment(), Ok("# foo \\".into()));
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, 0);
        }

        #[test]
        fn fails_with_unexpected_character() {
            let input = "[\n; bar";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(
                parser.parse_comment(),
                Err(ParseError {
                    line: 0,
                    col: 1,
                    msg: "expected comment, but found '['".into()
                })
            );
            assert_eq!(parser.column, old_pos);
        }
    }

    mod parse_entry {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let input = "KeyOne=value 1";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(
                parser.parse_entry(),
                Ok(("KeyOne".into(), "value 1".into()))
            );
            assert_eq!(parser.column, old_pos + 13);
        }

        #[test]
        fn test_with_no_value_succeeds() {
            let input = "KeyOne=";
            let mut parser = Parser::new(input);
            assert_eq!(parser.parse_entry(), Ok(("KeyOne".into(), "".into())));
        }
    }

    mod parse_key {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let input = "KeyOne";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(parser.parse_key(), Ok("KeyOne".into()));
            assert_eq!(parser.column, old_pos + 5);
            assert_eq!(parser.cur, None);
        }

        #[test]
        fn test_with_illegal_character_fails() {
            let input = "Key_One";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(
                parser.parse_key(),
                Err(ParseError {
                    line: 0,
                    col: 7,
                    msg: "Invalid key \"Key_One\". Allowed characters are A-Za-z0-9-".into()
                })
            );
            assert_eq!(parser.column, old_pos + 6);
            assert_eq!(parser.cur, None);
        }
    }

    mod parse_section {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let input = "[Section A]\nKeyOne=value 1";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_section(),
                Ok((
                    "Section A".into(),
                    vec![("KeyOne".into(), "value 1".into())],
                ))
            );
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, old_col + 13);
        }

        #[test]
        fn test_with_multiple_entries_succeeds() {
            let input = "[Section A]\nKeyOne=value 1\nKeyTwo=value 2";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_section(),
                Ok((
                    "Section A".into(),
                    vec![
                        ("KeyOne".into(), "value 1".into()),
                        ("KeyTwo".into(), "value 2".into()),
                    ],
                ))
            );
            assert_eq!(parser.line, old_line + 2);
            assert_eq!(parser.column, old_col + 13);
        }

        #[test]
        fn test_with_multiple_entries_with_same_key_succeeds() {
            let input = "[Section A]\nKeyOne=value 1\nKeyOne=value 2";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_section(),
                Ok((
                    "Section A".into(),
                    vec![
                        ("KeyOne".into(), "value 1".into()),
                        ("KeyOne".into(), "value 2".into()),
                    ],
                ))
            );
            assert_eq!(parser.line, old_line + 2);
            assert_eq!(parser.column, old_col + 13);
        }

        #[test]
        fn test_with_interspersed_comments_succeeds() {
            let input = "[Section A]\n# foo\nKeyOne=value 1\n; bar\nKeyOne=value 2\\\n#baz\nvalue 2 continued";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_section(),
                Ok((
                    "Section A".into(),
                    vec![
                        ("KeyOne".into(), "value 1".into()),
                        ("KeyOne".into(), "value 2 value 2 continued".into()),
                    ],
                ))
            );
            assert_eq!(parser.line, old_line + 6);
            assert_eq!(parser.column, old_col + 16);
        }

        #[test]
        fn test_with_extra_line_fails() {
            let input = "[Section A]\nKeyOne=value 1\nsome text";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_section(),
                Err(ParseError {
                    line: 2,
                    col: 6,
                    msg: "expected '=' after key, but found 't'".into()
                })
            );
            assert_eq!(parser.line, old_line + 2);
            assert_eq!(parser.column, old_col + 5);
        }

        #[test]
        fn test_without_kv_separator_fails() {
            let input = "[Section A]\nLooksLikeAKey";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_section(),
                Err(ParseError {
                    line: 1,
                    col: 13,
                    msg: "expected '=' after key, but found EOF".into()
                })
            );
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, old_col + 12);
        }
    }

    mod parse_section_header {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let input = "[Section A]";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(parser.parse_section_header(), Ok("Section A".into()));
            assert_eq!(parser.column, old_pos + 10);
        }

        #[test]
        fn test_needs_section_header_start() {
            let input = "Section A]";
            let mut parser = Parser::new(input);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError {
                    line: 0,
                    col: 1,
                    msg: "expected '[' as start of section header, but found 'S'".into()
                }),
            );
        }

        #[test]
        fn test_section_header_cannot_be_empty() {
            let input = "[]";
            let mut parser = Parser::new(input);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError {
                    line: 0,
                    col: 2,
                    msg: "section header cannot be empty".into()
                }),
            );
        }

        #[test]
        fn test_needs_section_header_end() {
            let input = "[Section A[";
            let mut parser = Parser::new(input);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError {
                    line: 0,
                    col: 11,
                    msg: "expected ']' as end of section header, but found EOF".into()
                }),
            );
        }

        #[test]
        fn test_early_eof_after_1() {
            let input = "[";
            let mut parser = Parser::new(input);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError {
                    line: 0,
                    col: 1,
                    msg: "expected ']' as end of section header, but found EOF".into()
                }),
            );
        }

        #[test]
        fn test_early_eof_after_2() {
            let input = "[Section A";
            let mut parser = Parser::new(input);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError {
                    line: 0,
                    col: 10,
                    msg: "expected ']' as end of section header, but found EOF".into()
                }),
            );
        }
    }

    mod parse_unit {
        use super::*;

        #[test]
        fn test_only_comments_should_create_empty_unit() {
            let input = "# foo\n; bar";
            let mut parser = Parser::new(input);
            assert_eq!(parser.parse_unit().ok(), Some(SystemdUnitData::new()),);
        }

        #[test]
        fn test_with_empty_section_succeeds() {
            let tokens = "[Section A]";
            let mut parser = Parser::new(tokens);

            let unit = parser.parse_unit().unwrap();
            assert_eq!(unit.len(), 1);

            let mut iter = unit.section_entries("Section A");
            assert_eq!(iter.next(), None);
        }

        #[test]
        fn test_with_section_with_entries_succeeds() {
            let tokens = "[Section A]\nKeyOne=value 1\nKeyTwo=value 2";
            let mut parser = Parser::new(tokens);

            let unit = parser.parse_unit().unwrap();
            assert_eq!(unit.len(), 1);

            let mut iter = unit.section_entries("Section A");
            assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
            assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
            assert_eq!(iter.next(), None);
        }

        #[test]
        fn test_with_multiple_sections_succeeds() {
            let tokens = "[Section A]\nKeyOne=value 1\n[Section B]\nKeyTwo=value 2";
            let mut parser = Parser::new(tokens);

            let unit = parser.parse_unit().unwrap();
            assert_eq!(unit.len(), 2);

            let mut iter = unit.section_entries("Section A");
            assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
            assert_eq!(iter.next(), None);

            let mut iter = unit.section_entries("Section B");
            assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
            assert_eq!(iter.next(), None);
        }

        #[test]
        fn test_with_same_section_occuring_mutlimple_times_succeeds() {
            let tokens = "[Section A]\nKeyOne=value 1\n[Section A]\nKeyTwo=value 2";
            let mut parser = Parser::new(tokens);

            let unit = parser.parse_unit().unwrap();
            assert_eq!(unit.len(), 1);

            let mut iter = unit.section_entries("Section A");
            assert_eq!(iter.next(), Some(("KeyOne", "value 1".into())));
            assert_eq!(iter.next(), Some(("KeyTwo", "value 2".into())));
            assert_eq!(iter.next(), None);
        }
    }

    mod parse_value {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let input = "value 1";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(parser.parse_value(), Ok("value 1".into()),);
            assert_eq!(parser.column, old_pos + 6);
        }

        #[test]
        fn test_with_empty_text_succeeds() {
            let input = "";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(parser.parse_value(), Ok(input.into()),);
            assert_eq!(parser.column, old_pos + 0);
        }

        #[test]
        fn test_with_multiple_spaces_succeeds() {
            let input = "this is some text";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(parser.parse_value(), Ok(input.into()),);
            assert_eq!(parser.column, old_pos + 16);
        }

        #[test]
        fn test_last_line_with_spaces_at_the_end_gets_trimmed() {
            let input = "this is \\\nsome \\\ntext   \t";
            let mut parser = Parser::new(input);
            let old_pos = parser.column;
            assert_eq!(
                parser.parse_value(),
                Ok("this is  some  text".trim_end().into()),
            );
            assert_eq!(parser.column, old_pos + 7);
        }

        #[test]
        fn test_turn_continuation_into_space() {
            let input = "this is some text\\\nmore text";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_value(),
                Ok("this is some text more text".into()),
            );
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, old_col + 8);
        }

        #[test]
        fn test_with_empty_line_continuations_succeeds() {
            let input = "\\\n\\\nlate text";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(parser.parse_value(), Ok("  late text".into()),);
            assert_eq!(parser.line, old_line + 2);
            assert_eq!(parser.column, old_col + 8);
        }

        #[test]
        fn test_leniency_with_space_after_line_continuation_succeeds() {
            let input = "foo \\    bar\\ \nbaz";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(parser.parse_value(), Ok("foo \\    bar baz".into()),);
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, old_col + 2);
        }

        #[test]
        fn test_with_interspersed_comments_suceeds() {
            let input = "some text\\\n# foo\n; bar\nmore text\\\n; baz\nsome more";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_value(),
                Ok("some text more text some more".into()),
            );
            assert_eq!(parser.line, old_line + 5);
            assert_eq!(parser.column, old_col + 8);
        }

        #[test]
        fn test_with_missing_line_after_contiuation_succeeds() {
            let input = "text\\\n# foo\n; bar";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(parser.parse_value(), Ok("text".into()),);
            assert_eq!(parser.line, old_line + 2);
            assert_eq!(parser.column, old_col + 4);
        }

        #[test]
        fn test_with_line_continuation_in_comment_succeeds() {
            let input = "foo\\\n#   -e HOST_WHITELIST= `#optional` \\\nbar";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(parser.parse_value(), Ok("foo bar".into()),);
            assert_eq!(parser.line, old_line + 2);
            assert_eq!(parser.column, old_col + 2);
        }

        #[test]
        fn test_with_new_section_after_continuation_succeeds() {
            let input = "text\\\n[";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(parser.parse_value(), Ok("text".into()),);
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, old_col + 0);
        }

        #[test]
        fn test_continuation_with_kv_style_line_succeeds() {
            let input = "org.foo.Arg1=arg1 \"org.foo.Arg2=arg 2\" \\\n  org.foo.Arg3=arg3";
            let mut parser = Parser::new(input);
            let old_line = parser.line;
            let old_col = parser.column;
            assert_eq!(
                parser.parse_value(),
                Ok("org.foo.Arg1=arg1 \"org.foo.Arg2=arg 2\"    org.foo.Arg3=arg3".into()),
            );
            assert_eq!(parser.line, old_line + 1);
            assert_eq!(parser.column, old_col + 18);
        }
    }
}
