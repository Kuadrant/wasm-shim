// Split a string at each non-escaped occurrence of a separator character.
pub fn tokenize_with_escaping(string: &str, separator: char, escape: char) -> Vec<String> {
    let mut token = String::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = string.chars();
    while let Some(ch) = chars.next() {
        match ch {
            x if x == separator => {
                tokens.push(token);
                token = String::new();
            }
            x if x == escape => {
                if let Some(next) = chars.next() {
                    token.push(next);
                }
            }
            _ => token.push(ch),
        }
    }
    tokens.push(token);
    tokens
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn tokenize_with_scaping_basic() {
        let string = r"one\.two..three\\\\.four\\\.\five.";
        let tokens = tokenize_with_escaping(string, '.', '\\');
        assert_eq!(tokens, vec!["one.two", "", r"three\\", r"four\.five", ""]);
    }

    #[test]
    fn tokenize_with_scaping_ends_with_separator() {
        let string = r"one.";
        let tokens = tokenize_with_escaping(string, '.', '\\');
        assert_eq!(tokens, vec!["one", ""]);
    }

    #[test]
    fn tokenize_with_scaping_ends_with_escape() {
        let string = r"one\";
        let tokens = tokenize_with_escaping(string, '.', '\\');
        assert_eq!(tokens, vec!["one"]);
    }

    #[test]
    fn tokenize_with_scaping_starts_with_separator() {
        let string = r".one";
        let tokens = tokenize_with_escaping(string, '.', '\\');
        assert_eq!(tokens, vec!["", "one"]);
    }

    #[test]
    fn tokenize_with_scaping_starts_with_escape() {
        let string = r"\one";
        let tokens = tokenize_with_escaping(string, '.', '\\');
        assert_eq!(tokens, vec!["one"]);
    }
}
