# Parser

This parser should be able to parse Systemd Unit files.
The format is described in https://www.freedesktop.org/software/systemd/man/systemd.syntax.html .
The syntax is inspired by [XDG Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/latest/) _.desktop_ files, which are in turn inspired by Microsoft Windows _.ini_ files.

## Grammar

This is a rough grammar extracted from the text in https://www.freedesktop.org/software/systemd/man/systemd.syntax.html :

> UNIT           = [COMMENT | SECTION]*
> COMMENT        = ('#' | ';') ANY* NL
> SECTION        = SECTION_HEADER [ENTRY]*
> SECTION_HEADER = '[' ANY+ ']' NL
> ENTRY          = KEY WS* '=' WS* VALUE NL
> KEY            = [A-Za-z0-9-]
> VALUE          = [QUOTE WS | ANY*]* CONTINUE_NL [VALUE | COMMENT] | [QUOTE | ANY*]* NL
> QUOTE          =  '"' QUOTE_DQ* '"' | '\'' QUOTE_SQ* '\''
> QUOTE_DQ       = [^"]* | [^"]* CONTINUE_NL QUOTE_DQ_MORE
> QUOTE_DQ_MORE  = COMMENT | QUOTE_DQ
> QUOTE_SQ       = [^']* | [^"]* CONTINUE_NL QUOTE_SQ_MORE
> QUOTE_SQ_MORE  = COMMENT | QUOTE_SQ
> ANY            = . <-- all characters except NL
> WS             = \s
> NL             = \n
> CONTINUE_NL    = '\' NL

Especially the '\' line continuations make things comlicated. :/

## Simplification

IMHO the grammar is a lot simpler when we filter the comments out first.
I.e. we split at '\n' and remove empty and comment lines.
After that we only have "significant" data to parse:

> UNIT           = SECTION*
> SECTION        = SECTION_HEADER [ENTRY]*
> SECTION_HEADER = '[' [^]]+ ']'NL
> ENTRY          = KEY WS* '=' WS* VALUE NL
> KEY            = [A-Za-z0-9-]
> VALUE          = [QUOTE WS | ANY*]* CONTINUE_NL VALUE | [QUOTE | ANY*]* NL
> QUOTE          =  '"' QUOTE_DQ* '"' | '\'' QUOTE_SQ* '\''
> QUOTE_DQ       = [^"]* | [^"]* CONTINUE_NL QUOTE_DQ
> QUOTE_SQ       = [^']* | [^']* CONTINUE_NL QUOTE_SQ
> ANY            = . <-- all characters except NL
> WS             = \s
> NL             = \n
> CONTINUE_NL    = '\' NL

We still can't get rid of line continuations though.