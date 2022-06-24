#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum TokenType {
    Comment, // #... or ;...
    SectionHeaderStart, // [
    SectionHeaderEnd,  // ]
    Text,
    KVSeparator, // =
    ContinueNL,  // \\
    EOF,
}

#[derive(Debug, PartialEq)]
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

#[derive(Debug, PartialEq)]
pub(crate) struct Lexer;

impl Lexer {
    pub(crate) fn tokens_from(data: &str) -> Result<Vec<Token>, super::ParseError> {
        let mut tokens: Vec<Token> = Vec::with_capacity(data.lines().count());

        for line in data.lines() {
            if line.is_empty() {
                continue;
            }
            if line.starts_with(&['#', ';']) {  // shortcut
                tokens.push(Token::new(TokenType::Comment, line));
                continue;
            }
            if tokens.last().map(|t| t.token_type != TokenType::ContinueNL).unwrap_or(true) && line.starts_with('[') && line.ends_with(']') {  // shortcut
                tokens.push(Token::new(TokenType::SectionHeaderStart, &line[0..1]));
                tokens.push(Token::new(TokenType::Text, &line[1..line.len()-1]));
                tokens.push(Token::new(TokenType::SectionHeaderEnd, &line[line.len()-1..line.len()]));
                continue;
            }
            if tokens.last().map(|t| t.token_type != TokenType::ContinueNL).unwrap_or(true) && line.contains("=") {
                if let Some((key, value)) = line.split_once("=") {
                    tokens.push(Token::new(TokenType::Text, key.trim_end()));
                    tokens.push(Token::new(TokenType::KVSeparator, "="));
                    if value.ends_with('\\') {
                        tokens.push(Token::new(TokenType::Text, &value[0..value.len()-1].trim_start()));
                        tokens.push(Token::new(TokenType::ContinueNL, &value[value.len()-1..value.len()]));
                    } else {
                        tokens.push(Token::new(TokenType::Text, value.trim_start()));
                    }
                }
                continue;
            } else {
                // TODO: we could check if any of the previous lines was a ContinueNL
                if line.ends_with('\\') {
                    tokens.push(Token::new(TokenType::Text, &line[0..line.len()-1]));
                    tokens.push(Token::new(TokenType::ContinueNL, &line[line.len()-1..line.len()]));
                } else {
                    tokens.push(Token::new(TokenType::Text, line));
                }
                continue;
            }
        }

        tokens.push(Token::new(TokenType::EOF, ""));

        Ok(tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod tokens_from {
        use super::*;

        #[test]
        fn test_should_always_end_with_eof_token() {
            let data = "


";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 1);
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }

        #[test]
        fn test_with_comments_succeeds() {
            let data = "
# foo

; bar";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 3);
            assert_eq!(tokens[0], Token::new(TokenType::Comment, "# foo"));
            assert_eq!(tokens[1], Token::new(TokenType::Comment, "; bar"));
        }

        #[test]
        fn test_with_section_header_succeeds() {
            let data = "[Section A]";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 4);
            assert_eq!(tokens[0], Token::new(TokenType::SectionHeaderStart, "["));
            assert_eq!(tokens[1], Token::new(TokenType::Text, "Section A"));
            assert_eq!(tokens[2], Token::new(TokenType::SectionHeaderEnd, "]"));
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }

        #[test]
        fn test_entry_with_whitespace_succeeds() {
            let data = "KeyOne = Something";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 4);
            assert_eq!(tokens[0], Token::new(TokenType::Text, "KeyOne"));
            assert_eq!(tokens[1], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[2], Token::new(TokenType::Text, "Something"));
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }

        #[test]
        fn test_entry_with_empty_value_succeeds() {
            let data = "KeyOne = ";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 4);
            assert_eq!(tokens[0], Token::new(TokenType::Text, "KeyOne"));
            assert_eq!(tokens[1], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[2], Token::new(TokenType::Text, ""));
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }

        #[test]
        fn test_entry_with_continuation_succeeds() {
            let data = "KeyOne = Something \\
Else";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 6);
            assert_eq!(tokens[0], Token::new(TokenType::Text, "KeyOne"));
            assert_eq!(tokens[1], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[2], Token::new(TokenType::Text, "Something "));
            assert_eq!(tokens[3], Token::new(TokenType::ContinueNL, "\\"));
            assert_eq!(tokens[4], Token::new(TokenType::Text, "Else"));
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }

        #[test]
        fn test_with_empty_line_continuations_succeeds() {
            let data = "KeyOne = \\
\\
late text";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 8);
            assert_eq!(tokens[0], Token::new(TokenType::Text, "KeyOne"));
            assert_eq!(tokens[1], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[2], Token::new(TokenType::Text, ""));
            assert_eq!(tokens[3], Token::new(TokenType::ContinueNL, "\\"));
            assert_eq!(tokens[4], Token::new(TokenType::Text, ""));
            assert_eq!(tokens[5], Token::new(TokenType::ContinueNL, "\\"));
            assert_eq!(tokens[6], Token::new(TokenType::Text, "late text"));
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }

        #[test]
        fn test_systemd_syntax_example_1_succeeds() {
            // see https://www.freedesktop.org/software/systemd/man/systemd.syntax.html#
            let data = "[Section A]
KeyOne=value 1
KeyTwo=value 2

# a comment

[Section B]
Setting=\"something\" \"some thing\" \"…\"
KeyTwo=value 2 \\
       value 2 continued

[Section C]
KeyThree=value 3\\
# this line is ignored
; this line is ignored too
       value 3 continued";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 32);
            assert_eq!(tokens[ 0], Token::new(TokenType::SectionHeaderStart, "["));
            assert_eq!(tokens[ 1], Token::new(TokenType::Text, "Section A"));
            assert_eq!(tokens[ 2], Token::new(TokenType::SectionHeaderEnd, "]"));
            assert_eq!(tokens[ 3], Token::new(TokenType::Text, "KeyOne"));
            assert_eq!(tokens[ 4], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[ 5], Token::new(TokenType::Text, "value 1"));
            assert_eq!(tokens[ 6], Token::new(TokenType::Text, "KeyTwo"));
            assert_eq!(tokens[ 7], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[ 8], Token::new(TokenType::Text, "value 2"));
            assert_eq!(tokens[ 9], Token::new(TokenType::Comment, "# a comment"));
            assert_eq!(tokens[10], Token::new(TokenType::SectionHeaderStart, "["));
            assert_eq!(tokens[11], Token::new(TokenType::Text, "Section B"));
            assert_eq!(tokens[12], Token::new(TokenType::SectionHeaderEnd, "]"));
            assert_eq!(tokens[13], Token::new(TokenType::Text, "Setting"));
            assert_eq!(tokens[14], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[15], Token::new(TokenType::Text, "\"something\" \"some thing\" \"…\""));
            assert_eq!(tokens[16], Token::new(TokenType::Text, "KeyTwo"));
            assert_eq!(tokens[17], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[18], Token::new(TokenType::Text, "value 2 "));
            assert_eq!(tokens[19], Token::new(TokenType::ContinueNL, "\\"));
            assert_eq!(tokens[20], Token::new(TokenType::Text, "       value 2 continued"));
            assert_eq!(tokens[21], Token::new(TokenType::SectionHeaderStart, "["));
            assert_eq!(tokens[22], Token::new(TokenType::Text, "Section C"));
            assert_eq!(tokens[23], Token::new(TokenType::SectionHeaderEnd, "]"));
            assert_eq!(tokens[24], Token::new(TokenType::Text, "KeyThree"));
            assert_eq!(tokens[25], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[26], Token::new(TokenType::Text, "value 3"));
            assert_eq!(tokens[27], Token::new(TokenType::ContinueNL, "\\"));
            assert_eq!(tokens[28], Token::new(TokenType::Comment, "# this line is ignored"));
            assert_eq!(tokens[29], Token::new(TokenType::Comment, "; this line is ignored too"));
            assert_eq!(tokens[30], Token::new(TokenType::Text, "       value 3 continued"));
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }

        #[test]
        fn  test_quadlet_annotatopm_container_case_succeeds() {
            let data = "## assert-podman-final-args imagename
## assert-podman-args \"--annotation\" \"org.foo.Arg0=arg0\"
## assert-podman-args \"--annotation\" \"org.foo.Arg1=arg1\"
## assert-podman-args \"--annotation\" \"org.foo.Arg2=arg 2\"
## assert-podman-args \"--annotation\" \"org.foo.Arg3=arg3\"

[Container]
Image=imagename
Annotation=org.foo.Arg1=arg1 \"org.foo.Arg2=arg 2\" \\
  org.foo.Arg3=arg3

Annotation=org.foo.Arg0=arg0
";

            let tokens = Lexer::tokens_from(data).unwrap();
            assert_eq!(tokens.len(), 20);
            assert_eq!(tokens[ 0], Token::new(TokenType::Comment, "## assert-podman-final-args imagename"));
            assert_eq!(tokens[ 1], Token::new(TokenType::Comment, "## assert-podman-args \"--annotation\" \"org.foo.Arg0=arg0\""));
            assert_eq!(tokens[ 2], Token::new(TokenType::Comment, "## assert-podman-args \"--annotation\" \"org.foo.Arg1=arg1\""));
            assert_eq!(tokens[ 3], Token::new(TokenType::Comment, "## assert-podman-args \"--annotation\" \"org.foo.Arg2=arg 2\""));
            assert_eq!(tokens[ 4], Token::new(TokenType::Comment, "## assert-podman-args \"--annotation\" \"org.foo.Arg3=arg3\""));
            assert_eq!(tokens[ 5], Token::new(TokenType::SectionHeaderStart, "["));
            assert_eq!(tokens[ 6], Token::new(TokenType::Text, "Container"));
            assert_eq!(tokens[ 7], Token::new(TokenType::SectionHeaderEnd, "]"));
            assert_eq!(tokens[ 8], Token::new(TokenType::Text, "Image"));
            assert_eq!(tokens[ 9], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[10], Token::new(TokenType::Text, "imagename"));
            assert_eq!(tokens[11], Token::new(TokenType::Text, "Annotation"));
            assert_eq!(tokens[12], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[13], Token::new(TokenType::Text, "org.foo.Arg1=arg1 \"org.foo.Arg2=arg 2\" "));
            assert_eq!(tokens[14], Token::new(TokenType::ContinueNL, "\\"));
            assert_eq!(tokens[15], Token::new(TokenType::Text, "  org.foo.Arg3=arg3"));
            assert_eq!(tokens[16], Token::new(TokenType::Text, "Annotation"));
            assert_eq!(tokens[17], Token::new(TokenType::KVSeparator, "="));
            assert_eq!(tokens[18], Token::new(TokenType::Text, "org.foo.Arg0=arg0"));
            assert_eq!(tokens.last().unwrap().token_type, TokenType::EOF);
        }
    }
}