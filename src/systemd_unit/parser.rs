pub(crate) mod lexer;

use std::{fmt::Display};

use self::lexer::{TokenType, Token};

type ParseResult<T> = Result<T, ParseError>;
#[derive(Debug, PartialEq)]
pub(crate) enum ParseError {
    LexingError(String),
    UnexpectedEOF(TokenType),
    UnexpectedToken(TokenType, TokenType),
}

impl Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LexingError(msg) =>
                write!(f, "LexingError: {:?}", msg),
            Self::UnexpectedEOF(expected) =>
                write!(f, "Unexpected End of File: Expected {:?}", expected),
            Self::UnexpectedToken(expected, found) =>
                write!(f, "Unexpected Token: Expected {:?}. Found {:?}.", expected, found),
        }
    }
}

pub(crate) struct Parser<'a> {
    tokens: Vec<Token<'a>>,
    pos: usize
}

// output types
#[derive(Debug, PartialEq)]
pub(crate) struct SystemdUnit {
    sections: Vec<Section>,
}

impl SystemdUnit {
    fn new() -> Self {
        SystemdUnit { sections: Vec::default() }
    }
}

#[derive(Debug, PartialEq)]
struct Section {
    header: SectionHeader,
    entries: Vec<Entry>,
}
type SectionHeader = String;
type Entry = (Key, Value);
type Key = String;
type Value = String;

impl<'a> Parser<'a> {
    pub fn new(tokens: Vec<Token<'a>>) -> Self {
        Self {
            tokens,
            pos: 0
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn is_match(&self, token_type: TokenType) -> bool {
        !self.is_eof() && self.peek().token_type == token_type
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

    // COMMENT        = ('#' | ';') ANY* NL
    fn parse_comment(&mut self) -> ParseResult<()> {
        let _ = self.take(TokenType::Comment)?;

        self.advance();
        Ok(())
    }

    fn parse_entry(&mut self) -> ParseResult<Entry> {
        todo!()
    }

    // SECTION        = SECTION_HEADER [ENTRY]*
    fn parse_section(&mut self) -> ParseResult<Section> {
        todo!()
    }

    fn parse_section_header(&mut self) -> ParseResult<SectionHeader> {
        let _ = self.take(TokenType::SectionHeaderStart)?;
        self.advance();

        let token = self.take(TokenType::Text)?;
        let section_header: SectionHeader = token.content.into();
        self.advance();

        let _ = self.take(TokenType::SectionHeaderEnd)?;
        self.advance();

        Ok(section_header)
    }

    // UNIT           = [COMMENT | SECTION]*
    fn parse_unit(&mut self) -> ParseResult<SystemdUnit> {
        let mut unit = SystemdUnit::new();

        while !self.is_eof() {
            // ignore comments
            let _ = self.parse_comment();

        //     let section = self.parse_section()?;

        //     unit.sections.push(section);
        }

        Ok(unit)
    }

    pub(crate) fn parse(&mut self) -> ParseResult<SystemdUnit> {
        self.parse_unit()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    mod parse_comment {
        use super::*;

        #[test]
        fn test_comment_consumes_token() {
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
    }

    mod parse_key {
        use super::*;
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
    }

    mod parse_section {
        use super::*;
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
    }
}