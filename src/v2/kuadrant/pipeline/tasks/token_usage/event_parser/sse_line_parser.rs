//
// Initially copied from: https://github.com/jpopesculian/eventsource-stream/blob/v0.2.3/src/parser.rs
// Original License: Apache-2.0

use nom::branch::alt;
use nom::bytes::streaming::{tag, take_while, take_while1, take_while_m_n};
use nom::combinator::opt;
use nom::sequence::{preceded, terminated};
use nom::IResult;
use nom::Parser;

/// ; ABNF definition from HTML spec
///
/// stream        = [ bom ] *event
/// event         = *( comment / field ) end-of-line
/// comment       = colon *any-char end-of-line
/// field         = 1*name-char [ colon [ space ] *any-char ] end-of-line
/// end-of-line   = ( cr lf / cr / lf )
///
/// ; characters
/// lf            = %x000A ; U+000A LINE FEED (LF)
/// cr            = %x000D ; U+000D CARRIAGE RETURN (CR)
/// space         = %x0020 ; U+0020 SPACE
/// colon         = %x003A ; U+003A COLON (:)
/// bom           = %xFEFF ; U+FEFF BYTE ORDER MARK
/// name-char     = %x0000-0009 / %x000B-000C / %x000E-0039 / %x003B-10FFFF
///                 ; a scalar value other than U+000A LINE FEED (LF), U+000D CARRIAGE RETURN (CR), or U+003A COLON (:)
/// any-char      = %x0000-0009 / %x000B-000C / %x000E-10FFFF
///                 ; a scalar value other than U+000A LINE FEED (LF) or U+000D CARRIAGE RETURN (CR)

#[derive(Debug, PartialEq)]
pub enum RawEventLine<'a> {
    Comment(&'a str),
    Field(&'a str, Option<&'a str>),
    Empty,
}

#[inline]
pub fn is_lf(c: char) -> bool {
    c == '\u{000A}'
}

#[inline]
fn is_cr(c: char) -> bool {
    c == '\u{000D}'
}

#[inline]
fn is_space(c: char) -> bool {
    c == '\u{0020}'
}

#[inline]
fn is_colon(c: char) -> bool {
    c == '\u{003A}'
}

#[inline]
fn is_bom(c: char) -> bool {
    c == '\u{feff}'
}

#[inline]
fn is_name_char(c: char) -> bool {
    matches!(c, '\u{0000}'..='\u{0009}'
        | '\u{000B}'..='\u{000C}'
        | '\u{000E}'..='\u{0039}'
        | '\u{003B}'..='\u{10FFFF}')
}

#[inline]
fn is_any_char(c: char) -> bool {
    matches!(c, '\u{0000}'..='\u{0009}'
        | '\u{000B}'..='\u{000C}'
        | '\u{000E}'..='\u{10FFFF}')
}

#[inline]
fn crlf(input: &str) -> IResult<&str, &str> {
    tag("\u{000D}\u{000A}")(input)
}

#[inline]
fn end_of_line(input: &str) -> IResult<&str, &str> {
    alt((
        crlf,
        take_while_m_n(1, 1, is_cr),
        take_while_m_n(1, 1, is_lf),
    ))
    .parse(input)
}

#[inline]
fn comment(input: &str) -> IResult<&str, RawEventLine<'_>> {
    preceded(
        take_while_m_n(1, 1, is_colon),
        terminated(take_while(is_any_char), end_of_line),
    )
    .parse(input)
    .map(|(input, comment)| (input, RawEventLine::Comment(comment)))
}

#[inline]
fn field(input: &str) -> IResult<&str, RawEventLine<'_>> {
    terminated(
        (
            take_while1(is_name_char),
            opt(preceded(
                take_while_m_n(1, 1, is_colon),
                preceded(opt(take_while_m_n(1, 1, is_space)), take_while(is_any_char)),
            )),
        ),
        end_of_line,
    )
    .parse(input)
    .map(|(input, (field, data))| (input, RawEventLine::Field(field, data)))
}

#[inline]
fn empty(input: &str) -> IResult<&str, RawEventLine<'_>> {
    end_of_line(input).map(|(i, _)| (i, RawEventLine::Empty))
}

pub fn line(input: &str) -> IResult<&str, RawEventLine<'_>> {
    alt((comment, field, empty)).parse(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_lf() {
        assert!(is_lf('\u{000A}'));
        assert!(is_lf('\n'));
        assert!(!is_lf('\r'));
        assert!(!is_lf(' '));
        assert!(!is_lf('a'));
    }

    #[test]
    fn test_is_cr() {
        assert!(is_cr('\u{000D}'));
        assert!(is_cr('\r'));
        assert!(!is_cr('\n'));
        assert!(!is_cr(' '));
        assert!(!is_cr('a'));
    }

    #[test]
    fn test_is_space() {
        assert!(is_space('\u{0020}'));
        assert!(is_space(' '));
        assert!(!is_space('\t'));
        assert!(!is_space('\n'));
        assert!(!is_space('a'));
    }

    #[test]
    fn test_is_colon() {
        assert!(is_colon('\u{003A}'));
        assert!(is_colon(':'));
        assert!(!is_colon(';'));
        assert!(!is_colon(' '));
        assert!(!is_colon('a'));
    }

    #[test]
    fn test_is_bom() {
        assert!(is_bom('\u{feff}'));
        assert!(!is_bom(' '));
        assert!(!is_bom('a'));
    }

    #[test]
    fn test_is_name_char() {
        // Valid name chars
        assert!(is_name_char('\u{0000}'));
        assert!(is_name_char('\u{0009}'));
        assert!(is_name_char('\u{000B}'));
        assert!(is_name_char('\u{000C}'));
        assert!(is_name_char('\u{000E}'));
        assert!(is_name_char('a'));
        assert!(is_name_char('Z'));
        assert!(is_name_char('0'));
        assert!(is_name_char('9'));
        assert!(is_name_char(';'));

        // Invalid name chars (LF, CR, colon)
        assert!(!is_name_char('\u{000A}')); // LF
        assert!(!is_name_char('\u{000D}')); // CR
        assert!(!is_name_char(':')); // colon
    }

    #[test]
    fn test_is_any_char() {
        // Valid any chars
        assert!(is_any_char('\u{0000}'));
        assert!(is_any_char('\u{0009}'));
        assert!(is_any_char('\u{000B}'));
        assert!(is_any_char('\u{000C}'));
        assert!(is_any_char('\u{000E}'));
        assert!(is_any_char('a'));
        assert!(is_any_char(':'));

        // Invalid any chars (LF, CR)
        assert!(!is_any_char('\u{000A}')); // LF
        assert!(!is_any_char('\u{000D}')); // CR
    }

    #[test]
    fn test_crlf() {
        assert_eq!(crlf("\r\n"), Ok(("", "\r\n")));
        assert_eq!(crlf("\r\nfoo"), Ok(("foo", "\r\n")));
        assert!(crlf("\n").is_err());
        assert!(crlf("\r").is_err());
        assert!(crlf("foo").is_err());
    }

    #[test]
    fn test_end_of_line() {
        // Streaming parsers need extra data to know parsing is complete
        assert_eq!(end_of_line("\r\nfoo"), Ok(("foo", "\r\n")));
        assert_eq!(end_of_line("\nfoo"), Ok(("foo", "\n")));
        assert_eq!(end_of_line("\rfoo"), Ok(("foo", "\r")));
        assert!(end_of_line("foo").is_err());
    }

    #[test]
    fn test_comment() {
        // Basic comment (streaming parsers need data after newline)
        let result = comment(":this is a comment\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Comment(c) => assert_eq!(c, "this is a comment"),
            _ => panic!("Expected Comment"),
        }

        // Empty comment
        let result = comment(":\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Comment(c) => assert_eq!(c, ""),
            _ => panic!("Expected Comment"),
        }

        // Comment with CRLF
        let result = comment(":comment\r\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Comment(c) => assert_eq!(c, "comment"),
            _ => panic!("Expected Comment"),
        }

        // Comment with remaining input
        let result = comment(":first\nremaining");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "remaining");
        match parsed {
            RawEventLine::Comment(c) => assert_eq!(c, "first"),
            _ => panic!("Expected Comment"),
        }

        // Not a comment (no colon)
        assert!(comment("not a comment\n").is_err());
    }

    #[test]
    fn test_field_with_value() {
        // Field with value and space (streaming parsers need data after newline)
        let result = field("event: message\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "event");
                assert_eq!(value, "message");
            }
            _ => panic!("Expected Field with value"),
        }

        // Field with value without space
        let result = field("data:hello world\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "data");
                assert_eq!(value, "hello world");
            }
            _ => panic!("Expected Field with value"),
        }

        // Field with empty value after colon and space
        let result = field("retry: \nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "retry");
                assert_eq!(value, "");
            }
            _ => panic!("Expected Field with empty value"),
        }

        // Field with empty value after colon (no space)
        let result = field("id:\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "id");
                assert_eq!(value, "");
            }
            _ => panic!("Expected Field with empty value"),
        }
    }

    #[test]
    fn test_field_without_value() {
        // Field without colon (streaming parsers need data after newline)
        let result = field("event\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, None) => {
                assert_eq!(name, "event");
            }
            _ => panic!("Expected Field without value"),
        }
    }

    #[test]
    fn test_field_with_different_line_endings() {
        // CRLF (streaming parsers need data after line ending)
        let result = field("data:test\r\nnext");
        assert!(result.is_ok());
        match result.unwrap().1 {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "data");
                assert_eq!(value, "test");
            }
            _ => panic!("Expected Field"),
        }

        // CR
        let result = field("data:test\rnext");
        assert!(result.is_ok());
        match result.unwrap().1 {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "data");
                assert_eq!(value, "test");
            }
            _ => panic!("Expected Field"),
        }

        // LF
        let result = field("data:test\nnext");
        assert!(result.is_ok());
        match result.unwrap().1 {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "data");
                assert_eq!(value, "test");
            }
            _ => panic!("Expected Field"),
        }
    }

    #[test]
    fn test_empty() {
        // Streaming parsers need data after line ending
        assert_eq!(empty("\nremaining"), Ok(("remaining", RawEventLine::Empty)));
        assert_eq!(
            empty("\r\nremaining"),
            Ok(("remaining", RawEventLine::Empty))
        );
        assert_eq!(empty("\rremaining"), Ok(("remaining", RawEventLine::Empty)));
        assert!(empty("not empty\n").is_err());
    }

    #[test]
    fn test_line_comment() {
        let result = line(":comment\nnext");
        assert!(result.is_ok());
        match result.unwrap().1 {
            RawEventLine::Comment(c) => assert_eq!(c, "comment"),
            _ => panic!("Expected Comment"),
        }
    }

    #[test]
    fn test_line_field() {
        let result = line("event:message\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "event");
                assert_eq!(value, "message");
            }
            _ => panic!("Expected Field"),
        }
    }

    #[test]
    fn test_line_empty() {
        let result = line("\nnext");
        assert!(result.is_ok());
        match result.unwrap().1 {
            RawEventLine::Empty => {}
            _ => panic!("Expected Empty"),
        }
    }

    #[test]
    fn test_line_priority() {
        // Comment should be parsed before field
        // (both start with valid characters, but comment needs colon at start)
        let result = line(":not a field\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Comment(_) => {}
            _ => panic!("Expected Comment to have priority"),
        }
    }

    #[test]
    fn test_field_with_special_characters() {
        // Field name with numbers
        let result = field("retry3000\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, None) => assert_eq!(name, "retry3000"),
            _ => panic!("Expected Field"),
        }

        // Field value with colons
        let result = field("data:time:12:30:45\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "data");
                assert_eq!(value, "time:12:30:45");
            }
            _ => panic!("Expected Field"),
        }
    }

    #[test]
    fn test_multiple_spaces_after_colon() {
        // Only the first space after colon should be stripped
        let result = field("data:  multiple spaces\nnext");
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "data");
                assert_eq!(value, " multiple spaces"); // First space stripped, rest preserved
            }
            _ => panic!("Expected Field"),
        }
    }

    #[test]
    fn test_line_incomplete_data() {
        let mut buf = String::from("data: {\"id");
        let result = line(buf.as_str());
        assert!(result.is_err());
        let e = result.unwrap_err();
        assert!(e.is_incomplete());

        buf.push_str("\":1}\nnext");
        let result = line(buf.as_str());
        assert!(result.is_ok());
        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "next");
        match parsed {
            RawEventLine::Field(name, Some(value)) => {
                assert_eq!(name, "data");
                assert_eq!(value, r#"{"id":1}"#);
            }
            _ => panic!("Expected Field"),
        }
    }
}
