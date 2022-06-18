#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum TokenType {
    Comment, // #... or ;...
    SectionHeaderStart, // [
    SectionHeaderEnd,  // ]
    Text,
    KVSeparator, // =
    NL,  // \n
    WS,  // \s+
    ContinueNL,  // \\\n
    QuoteDouble,  // "
    QuoteSingle,  // '
    //EscapeSequence, // e.g. "\a"
    EOF,
}

#[derive(Debug)]
pub(crate) struct Token<'a> {
    pub(crate) token_type: TokenType,
    pub(crate) content: &'a str,
}

impl<'a> Token<'a> {
    pub(crate) fn new(token_type: TokenType, content: &'a str) -> Self {
        Self {
            token_type,
            content
        }
    }
}

pub(crate) struct Lexer;

impl Lexer {
    pub(crate) fn tokens_from(data: &str) -> Vec<Token> {
        Vec::new()
    }
}