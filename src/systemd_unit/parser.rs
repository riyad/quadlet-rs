pub(crate) mod lexer;

use std::{fmt::Display};

use self::lexer::{TokenType, Token};
use super::{SystemdUnit, Entry, Section};

type ParseResult<T> = Result<T, ParseError>;
#[derive(Debug, PartialEq)]
pub(crate) enum ParseError {
    CannotContinue(String),
    InvalidKey(String),
    LexingError(String),
    UnexpectedEOF(TokenType),
    UnexpectedToken(TokenType, TokenType),
}

impl Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CannotContinue(msg) =>
                write!(f, "{msg:?}"),
            Self::InvalidKey(key) =>
                write!(f, "Invalid key {key:?}. Allowed characters are A-Za-z0-9-"),
            Self::LexingError(msg) =>
                write!(f, "LexingError: {msg:?}"),
            Self::UnexpectedEOF(expected) =>
                write!(f, "Unexpected End of File: Expected {expected:?}"),
            Self::UnexpectedToken(expected, found) =>
                write!(f, "Unexpected Token: Expected {expected:?}. Found {found:?}."),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Parser<'a> {
    tokens: Vec<Token<'a>>,
    pos: usize
}

impl<'a> Parser<'a> {
    pub fn new(tokens: Vec<Token<'a>>) -> Self {
        Self {
            tokens,
            pos: 0
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.tokens.len() || self.peek().token_type == TokenType::EOF
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn take(&self, expected_token_type: TokenType) -> Result<&Token, ParseError> {
        if self.is_eof() {
            return Err(ParseError::UnexpectedEOF(expected_token_type));
        }
        let token = self.peek();
        if token.token_type != expected_token_type {
            return Err(ParseError::UnexpectedToken(
                expected_token_type,
                token.token_type
            ))
        }
        Ok(token)
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    pub(crate) fn parse(&mut self) -> ParseResult<SystemdUnit> {
        self.parse_unit()
    }

    // COMMENT        = ('#' | ';') ANY* NL
    fn parse_comment(&mut self) -> ParseResult<()> {
        let _ = self.take(TokenType::Comment)?;

        self.advance();
        Ok(())
    }

    // ENTRY          = KEY WS* '=' WS* VALUE NL
    // NOTE: whitespace around '=' has already been stripped in the lexer
    fn parse_entry(&mut self) -> ParseResult<Entry> {
        let key = self.parse_key()?;

        let _ = self.take(TokenType::KVSeparator)?;
        self.advance();

        let value = self.parse_value()?;

        Ok((key, value))
    }

    // KEY            = [A-Za-z0-9-]
    fn parse_key(&mut self) -> ParseResult<String> {
        let key: String = self.take(TokenType::Text)?.content.into();
        if !key.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
            return Err(ParseError::InvalidKey(key.into()))
        }
        self.advance();

        Ok(key)
    }

    // SECTION        = SECTION_HEADER [COMMENT | ENTRY]*
    fn parse_section(&mut self) -> ParseResult<Section> {
        let name = self.parse_section_header()?;

        let mut section = Section {
            name: name,
            entries: Vec::default(),
        };

        while !self.is_eof() {
            match self.peek().token_type {
                TokenType::Comment => {
                    // ignore comment
                    let _ = self.parse_comment();
                },
                TokenType::Text => {
                    match self.parse_entry() {
                        Ok(entry) => section.entries.push(entry),
                        Err(ParseError::CannotContinue(_)) => {},
                        Err(e) => return Err(e),
                    }
                },
                _ => break,
            }
        }

        Ok(section)
    }

    // SECTION_HEADER = '[' ANY+ ']' NL
    fn parse_section_header(&mut self) -> ParseResult<String> {
        let _ = self.take(TokenType::SectionHeaderStart)?;
        self.advance();

        let token = self.take(TokenType::Text)?;
        let section_name = token.content.into();
        self.advance();

        let _ = self.take(TokenType::SectionHeaderEnd)?;
        self.advance();

        Ok(section_name)
    }

    // UNIT           = [COMMENT | SECTION]*
    fn parse_unit(&mut self) -> ParseResult<SystemdUnit> {
        let mut unit = SystemdUnit::new();

        while !self.is_eof() {
            match self.peek().token_type {
                TokenType::Comment => {
                    // ignore comment
                    let _ = self.parse_comment();
                },
                TokenType::SectionHeaderStart => {
                    match self.parse_section() {
                        Ok(section) => unit.sections.push(section),
                        Err(ParseError::CannotContinue(_)) => {},
                        Err(e) => return Err(e),
                    }
                },
                _ => return Err(ParseError::CannotContinue("Expected comment or section".into())),
            };
        }

        Ok(unit)
    }

    // VALUE          = [QUOTE WS | ANY*]* CONTINUE_NL [COMMENT]* VALUE | [QUOTE | ANY*]* NL
    // NOTE: this is not what the code does ATM (at all)!
    // more like: VALUE = ANY* CONTINUE_NL [COMMENT]* VALUE
    fn parse_value(&mut self) -> ParseResult<String> {
        let mut value: String = self.take(TokenType::Text)?.content.into();
        self.advance();

        if !self.is_eof() {
            match self.peek().token_type {
                TokenType::ContinueNL => {
                    self.advance();

                    while !self.is_eof() {
                        match self.parse_comment() {
                            Ok(_) => {},
                            Err(_) => break,
                        }
                    }

                    let more_value = self.parse_value()?;
                    value += format!(" {more_value}").as_str();

                },
                _ => {},
            }
        }

        // TODO: parse quotes
        // TODO: parse escape sequences

        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_comment {
        use super::*;

        #[test]
        fn test_success_consumes_token() {
            let tokens = vec![
                Token::new(TokenType::Comment, "# foo"),
                Token::new(TokenType::Comment, "; bar"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(parser.parse_comment(), Ok(()));
            assert_eq!(parser.pos, old_pos+1);
        }

        #[test]
        fn test_error_does_not_consume_token() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Comment, "; bar"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(parser.parse_comment(), Err(ParseError::UnexpectedToken(TokenType::Comment, TokenType::SectionHeaderStart)));
            assert_eq!(parser.pos, old_pos);
        }
    }

    mod parse_entry {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let tokens = vec![
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_entry(),
                Ok(("KeyOne".into(), "value 1".into()))
            );
            assert_eq!(parser.pos, old_pos+3);
        }

        #[test]
        fn test_with_no_value_succeeds() {
            let tokens = vec![
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, ""),  // the lexer does this thankfully
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_entry(),
                Ok(("KeyOne".into(), "".into()))
            );
            assert_eq!(parser.pos, old_pos+3);
        }
    }

    mod parse_key {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let tokens = vec![
                Token::new(TokenType::Text, "KeyOne"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_key(),
                Ok("KeyOne".into())
            );
            assert_eq!(parser.pos, old_pos+1);
        }

        #[test]
        fn test_with_illegal_character_fails() {
            let tokens = vec![
                Token::new(TokenType::Text, "Key_One"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_key(),
                Err(ParseError::InvalidKey("Key_One".into()))
            );
            assert_eq!(parser.pos, old_pos);
        }
    }

    mod parse_unit {
        use super::*;

        #[test]
        fn test_only_comments_should_create_empty_unit() {
            let tokens = vec![
                Token::new(TokenType::Comment, "# foo"),
                Token::new(TokenType::Comment, "; bar"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(parser.parse_unit(), Ok(SystemdUnit { sections: Vec::new() }))
        }

        #[test]
        fn test_with_empty_section_succeeds() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_unit(),
                Ok(SystemdUnit {
                    sections:   vec![
                        Section {
                            name: "Section A".into(),
                            entries: vec![],
                        },
                    ]
                })
            );
        }

        #[test]
        fn test_with_section_with_entries_succeeds() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
                Token::new(TokenType::Text, "KeyTwo"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 2"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_unit(),
                Ok(SystemdUnit {
                    sections:   vec![
                        Section {
                            name: "Section A".into(),
                            entries: vec![
                                ("KeyOne".into(), "value 1".into()),
                                ("KeyTwo".into(), "value 2".into()),
                            ],
                        },
                    ]
                })
            );
        }


        #[test]
        fn test_with_multiple_sections_succeeds() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section B"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyTwo"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 2"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_unit(),
                Ok(SystemdUnit {
                    sections:   vec![
                        Section {
                            name: "Section A".into(),
                            entries: vec![
                                ("KeyOne".into(), "value 1".into()),
                            ],
                        },
                        Section {
                            name: "Section B".into(),
                            entries: vec![
                                ("KeyTwo".into(), "value 2".into()),
                            ],
                        },
                    ]
                })
            );
        }

        #[test]
        fn test_with_same_section_occuring_mutlimple_times_succeeds() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyTwo"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 2"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_unit(),
                Ok(SystemdUnit {
                    sections:   vec![
                        Section {
                            name: "Section A".into(),
                            entries: vec![
                                ("KeyOne".into(), "value 1".into()),
                            ],
                        },
                        Section {
                            name: "Section A".into(),
                            entries: vec![
                                ("KeyTwo".into(), "value 2".into()),
                            ],
                        },
                    ]
                })
            );
        }
    }

    mod parse_section {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_section(),
                Ok(Section{
                    name: "Section A".into(),
                    entries: vec![("KeyOne".into(), "value 1".into())],
                })
            );
            assert_eq!(parser.pos, old_pos+6);
        }

        #[test]
        fn test_with_multiple_entries_succeeds() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
                Token::new(TokenType::Text, "KeyTwo"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 2"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_section(),
                Ok(Section{
                    name: "Section A".into(),
                    entries: vec![
                        ("KeyOne".into(), "value 1".into()),
                        ("KeyTwo".into(), "value 2".into()),
                    ],
                })
            );
            assert_eq!(parser.pos, old_pos+9);
        }

        #[test]
        fn test_with_multiple_entries_with_same_key_succeeds() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 2"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_section(),
                Ok(Section{
                    name: "Section A".into(),
                    entries: vec![
                        ("KeyOne".into(), "value 1".into()),
                        ("KeyOne".into(), "value 2".into()),
                    ],
                })
            );
            assert_eq!(parser.pos, old_pos+9);
        }

        #[test]
        #[ignore]
        fn test_with_interspersed_comments_succeeds() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Comment, "# foo"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
                Token::new(TokenType::Comment, "; bar"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 2"),
                Token::new(TokenType::ContinueNL, "\\"),
                Token::new(TokenType::Comment, "# baz"),
                Token::new(TokenType::Text, "value 2 continued"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_section(),
                Ok(Section{
                    name: "Section A".into(),
                    entries: vec![
                        ("KeyOne".into(), "value 1".into()),
                        ("KeyOne".into(), "value 2 value 2 continued".into()),
                    ],
                })
            );
            assert_eq!(parser.pos, old_pos+14);
        }

        #[test]
        fn test_with_extra_line_fails() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "KeyOne"),
                Token::new(TokenType::KVSeparator, "="),
                Token::new(TokenType::Text, "value 1"),
                Token::new(TokenType::Text, "some text"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_section(),
                Err(ParseError::InvalidKey("some text".into()))
            );
            assert_eq!(parser.pos, old_pos+6);
        }

        #[test]
        fn test_without_kv_separator_fails() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
                Token::new(TokenType::Text, "LooksLikeAKey"),
                // KVSeparator missing
                Token::new(TokenType::Text, "Looks Like A Value"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_section(),
                Err(ParseError::UnexpectedToken(TokenType::KVSeparator, TokenType::Text))
            );
            assert_eq!(parser.pos, old_pos+4);
        }
    }

    mod parse_section_header {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(parser.parse_section_header(), Ok("Section A".into()));
            assert_eq!(parser.pos, old_pos+3);
        }

        #[test]
        fn test_needs_section_header_start() {
            let tokens = vec![
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderEnd, "]"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError::UnexpectedToken(TokenType::SectionHeaderStart, TokenType::Text))
            );
        }

        #[test]
        fn test_section_header_cannot_be_empty() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::SectionHeaderEnd, "]"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError::UnexpectedToken(TokenType::Text, TokenType::SectionHeaderEnd))
            );
        }

        #[test]
        fn test_needs_section_header_end() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
                Token::new(TokenType::SectionHeaderStart, "["),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError::UnexpectedToken(TokenType::SectionHeaderEnd, TokenType::SectionHeaderStart))
            );
        }

        #[test]
        fn test_early_eof_after_1() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError::UnexpectedEOF(TokenType::Text))
            );
        }

        #[test]
        fn test_early_eof_after_2() {
            let tokens = vec![
                Token::new(TokenType::SectionHeaderStart, "["),
                Token::new(TokenType::Text, "Section A"),
            ];
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_section_header(),
                Err(ParseError::UnexpectedEOF(TokenType::SectionHeaderEnd))
            );
        }
    }

    mod parse_value {
        use super::*;

        #[test]
        fn test_success_consumes_tokens() {
            let tokens = vec![
                Token::new(TokenType::Text, "value 1"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_value(),
                Ok("value 1".into())
            );
            assert_eq!(parser.pos, old_pos+1);
        }

        #[test]
        fn test_with_empty_text_succeeds() {
            let tokens = vec![
                Token::new(TokenType::Text, ""),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_value(),
                Ok("".into())
            );
            assert_eq!(parser.pos, old_pos+1);
        }

        #[test]
        fn test_with_multiple_spaces_succeeds() {
            let tokens = vec![
                Token::new(TokenType::Text, "this is some text"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_value(),
                Ok("this is some text".into())
            );
            assert_eq!(parser.pos, old_pos+1);
        }

        #[test]
        fn test_turn_continuation_into_space() {
            let tokens = vec![
                Token::new(TokenType::Text, "this is some text"),
                Token::new(TokenType::ContinueNL, "\\"),
                Token::new(TokenType::Text, "more text"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_value(),
                Ok("this is some text more text".into())
            );
            assert_eq!(parser.pos, old_pos+3);
        }

        #[test]
        fn test_with_empty_line_continuations_succeeds() {
            let tokens = vec![
                Token::new(TokenType::Text, ""),
                Token::new(TokenType::ContinueNL, "\\"),
                Token::new(TokenType::Text, ""),
                Token::new(TokenType::ContinueNL, "\\"),
                Token::new(TokenType::Text, "late text"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_value(),
                Ok("  late text".into())
            );
            assert_eq!(parser.pos, old_pos+5);
        }

        #[test]
        fn test_with_interspersed_comments_suceeds() {
            let tokens = vec![
                Token::new(TokenType::Text, "some text"),
                Token::new(TokenType::ContinueNL, "\\"),
                Token::new(TokenType::Comment, "# foo"),
                Token::new(TokenType::Comment, "; bar"),
                Token::new(TokenType::Text, "more text"),
                Token::new(TokenType::ContinueNL, "\\"),
                Token::new(TokenType::Comment, "; baz"),
                Token::new(TokenType::Text, "some more"),
            ];
            let mut parser = Parser::new(tokens);
            let old_pos = parser.pos;
            assert_eq!(
                parser.parse_value(),
                Ok("some text more text some more".into())
            );
            assert_eq!(parser.pos, old_pos+8);
        }
    }
}