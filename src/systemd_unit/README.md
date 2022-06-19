# Parser

This parser should be able to parse Systemd Unit files.
The format is described in https://www.freedesktop.org/software/systemd/man/systemd.syntax.html .
The syntax is inspired by [XDG Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/latest/) _.desktop_ files, which are in turn inspired by Microsoft Windows _.ini_ files.

## Grammar

This is a rough grammar extracted from the text in https://www.freedesktop.org/software/systemd/man/systemd.syntax.html :

> UNIT           = [COMMENT | SECTION]*
> COMMENT        = ('#' | ';') ANY* NL
> SECTION        = SECTION_HEADER [COMMENT | ENTRY]*
> SECTION_HEADER = '[' ANY+ ']' NL
> ENTRY          = KEY WS* '=' WS* VALUE NL
> KEY            = [A-Za-z0-9-]
> VALUE          = ANY* CONTINUE_NL [COMMENT]* VALUE
> ANY            = . <-- all characters except NL
> WS             = \s
> NL             = \n
> CONTINUE_NL    = '\' NL

Especially the '\' line continuations make things complicated. :/

## Quotes

Quoting is only allowed for certain settings.
For optional unquoting we can extract the following grammar from the text in https://www.freedesktop.org/software/systemd/man/systemd.syntax.html#Quoting .

> VALUE_TEXT = ITEM [WS ITEM]* NL
> ITEM       = QUOTE | [^WS]*
> QUOTE      = '"' (ESCAPE_SEQ | [^"])* '"' | '\'' (ESCAPE_SEQ | [^'])* '\''
> ESCAPE_SEQ = '\' ([abfnrtv\"'s] | 'x' HEX{2} | 'u' HEX{4} | 'U' HEX{8} )
> WS         = \s