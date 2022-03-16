// A simple globbing pattern implementation based on regular expressions.
//
// The only characters taken into account are:
//
// - '?': 0 or 1 characters
// - '*': 0 or more characters
// - '+': 1 or more characters
//
use std::convert::TryFrom;

use regex::{Regex, RegexSet};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to create regular expression")]
    Regex(#[from] regex::Error),
}

#[repr(transparent)]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct GlobPattern(Regex);

impl<'a> TryFrom<&'a str> for GlobPattern {
    type Error = Error;

    #[inline]
    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<String> for GlobPattern {
    type Error = Error;

    #[inline]
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<GlobPattern> for String {
    fn from(g: GlobPattern) -> Self {
        g.into_inner().to_string()
    }
}

impl GlobPattern {
    pub fn new(pattern: &str) -> Result<Self, Error> {
        let regex_pattern = Self::glob_pattern(pattern);

        Ok(Self(Regex::new(regex_pattern.as_str())?))
    }

    pub fn is_match(&self, s: &str) -> bool {
        self.0.is_match(s)
    }

    pub fn regex(&self) -> &Regex {
        &self.0
    }

    pub fn into_inner(self) -> Regex {
        self.0
    }

    pub fn glob_pattern(pattern: &str) -> String {
        let escaped_pattern = regex::escape(pattern);
        let regex_pattern = regex_unescape_specials(&['*', '?', '+'], escaped_pattern.as_str());

        format!(r"\A{}\z", regex_pattern.as_str())
    }
}

// Unescape previously escaped string with special characters.
// Special characters have to be meaningful regex characters needing escaping.
fn regex_unescape_specials(specials: &[char], regex_s: &str) -> String {
    enum Escaped {
        Nope,
        One,
        Preceded,    // One -> Preceded => printed a slash
        OnePreceded, // Preceded -> OnePreceded => got a slash after an emitted slash
    }

    impl Escaped {
        pub fn push(&mut self, specials: &[char], c: char, s: &mut String) {
            if c != '\\' && !specials.contains(&c) {
                if let Self::One = self {
                    s.push('\\');
                }
                s.push(c);
                *self = Self::Nope;
                return;
            }

            *self = match (c, &self) {
                ('\\', Self::Nope) => Self::One,
                ('\\', Self::One) => {
                    s.push('\\');
                    Self::Preceded
                }
                ('\\', Self::OnePreceded) => {
                    s.push('\\');
                    Self::Nope
                }
                ('\\', Self::Preceded) => Self::OnePreceded,
                (special, Self::One) | (special, Self::Preceded) => {
                    s.push('.');
                    s.push(special);
                    Self::Nope
                }
                (special, Self::OnePreceded) => {
                    s.push(special);
                    Self::Nope
                }
                (_, Self::Nope) => panic!("this string should have been escaped"),
            };
        }
    }

    let mut escaped = Escaped::Nope;

    regex_s
        .chars()
        .fold(String::with_capacity(regex_s.len()), |mut acc, c| {
            escaped.push(specials, c, &mut acc);
            acc
        })
}

#[repr(transparent)]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct GlobPatternSet(RegexSet);

impl TryFrom<Vec<String>> for GlobPatternSet {
    type Error = Error;

    fn try_from(v: Vec<String>) -> Result<Self, Self::Error> {
        Self::new(v.iter().map(|pat| GlobPattern::glob_pattern(pat.as_str())))
    }
}

impl<'a> TryFrom<&'a str> for GlobPatternSet {
    type Error = Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::new(core::iter::once(GlobPattern::glob_pattern(value)))
    }
}

impl From<GlobPatternSet> for Vec<String> {
    fn from(gs: GlobPatternSet) -> Self {
        gs.0.patterns().to_vec()
    }
}
// The default value will match everything
impl Default for GlobPatternSet {
    fn default() -> Self {
        // panic: won't panic since "*" is always valid
        Self::new(["*"].iter()).unwrap()
    }
}

impl GlobPatternSet {
    pub fn new<I, S>(exprs: I) -> Result<Self, Error>
    where
        S: AsRef<str>,
        I: IntoIterator<Item = S>,
    {
        Ok(Self(RegexSet::new(exprs)?))
    }

    pub fn is_match(&self, s: &str) -> bool {
        self.0.is_match(s)
    }

    pub fn regex_set(&self) -> &RegexSet {
        &self.0
    }

    pub fn into_inner(self) -> RegexSet {
        self.0
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn glob_pattern_matches() -> Result<(), Error> {
        let fixtures = [
            (r"???", r"...", true),
            (r"...", r"???", false),
            (r"...", r"...", true),
            (r"\.\.\.", r"...", true),
            (r"\\.\\.\\.", r"\.\.\.", true),
            // do not use this for paths, because it does not take into account directories
            (r"C:\\*\\calc*.exe", r"C:\User\Alex\calc.exe", true),
            (
                r"the_asterisk_is_the_\*_character",
                r"the_asterisk_is_the_*_character",
                true,
            ),
            (
                r"the_asterisk_is_the_\*_character",
                r"the_asterisk_is_the_?_character",
                false,
            ),
            (
                r"midpoint_*_not_important",
                r"midpoint_0000_not_important",
                true,
            ),
            (r"anything_goes_after_*", r"anything_goes_after_", true),
            (r"*_anything_goes_before", r"_anything_goes_before", true),
            (
                r"*_anything_goes_before",
                r"sa786dn _anything_goes_before",
                true,
            ),
            (
                r"the_question_mark_can_work_as_a_?_character",
                r"the_question_mark_can_work_as_a_?_character",
                true,
            ),
            (
                r"the_question_mark_can_work_as_a_?_character",
                r"the_question_mark_can_work_as_a_!_character",
                true,
            ),
            (
                r"the_question_mark_can_work_as_a_?_character",
                r"the_question_mark_can_work_as_a_??_character",
                false,
            ),
            (
                r"anything_goes_after_*.",
                r"anything_goes_after_127jhas89,.",
                true,
            ),
            (r"match_one_of_more_+", r"match_one_of_more_123", true),
            (r"match_one_of_more_+.", r"match_one_of_more_.", false),
            (
                r"I_want_to_match_slashes_only:_1.\\_2.\\\\_3.\\\\\\_",
                r"I_want_to_match_slashes_only:_1.\_2.\\_3.\\\_",
                true,
            ),
            (
                r"I_want_to_match_mixed_use:_1.\*_2.\\*_3.\\\*_",
                r"I_want_to_match_mixed_use:_1.*_2.\hello_3.\*_",
                true,
            ),
        ];

        for &(pattern, input, expected) in &fixtures {
            let glob = GlobPattern::new(pattern)?;
            assert_eq!(glob.is_match(input), expected);
        }

        Ok(())
    }

    mod unescape_logic {
        use super::*;

        #[test]
        fn test_regex_unescape_specials() {
            let fixtures = [
                (
                    r"_*_\*_\\*_\\\*_\\\\*_\\\\\*_\\\\\\*_\\\\\\\*",
                    r"_.*_\*_\\.*_\\\*_\\\\.*_\\\\\*_\\\\\\.*_\\\\\\\*",
                ),
                (
                    r"*_\*_\\*_\\\*_\\\\*_\\\\\*_\\\\\\*_\\\\\\\*",
                    r".*_\*_\\.*_\\\*_\\\\.*_\\\\\*_\\\\\\.*_\\\\\\\*",
                ),
                (r"\*_i*_\*_i*_*", r"\*_i.*_\*_i.*_.*"),
                (
                    r"the_asterisk_is_the_\\*_character",
                    r"the_asterisk_is_the_\\.*_character",
                ),
                (r"just_a_slash_\\_", r"just_a_slash_\\_"),
                (r"just_a_slash_\\", r"just_a_slash_\\"),
                (r"just_slashes_\\\\", r"just_slashes_\\\\"),
                (
                    r"the_asterisk_is_the_\*_character",
                    r"the_asterisk_is_the_\*_character",
                ),
            ];

            for &(input, expected) in &fixtures {
                let escaped = regex::escape(input);
                let regex_s = regex_unescape_specials(&['*'], &escaped);
                assert_eq!(regex_s.as_str(), expected);
            }
        }
    }
}
