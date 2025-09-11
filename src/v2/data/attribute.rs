use chrono::{DateTime, FixedOffset};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

#[derive(Debug)]
pub struct PropError {
    message: String,
}

impl Display for PropError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "PropError {{ message: {:?} }}", self.message)
    }
}

impl Error for PropError {}

impl PropError {
    pub fn new(message: String) -> PropError {
        PropError { message }
    }
}

pub trait AttributeValue {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError>
    where
        Self: Sized;
}

impl AttributeValue for String {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        String::from_utf8(raw_attribute).map_err(|err| {
            PropError::new(format!(
                "parse: failed to parse selector String value, error: {err}"
            ))
        })
    }
}

impl AttributeValue for i64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(i64::from_le_bytes(bytes)),
            Err(_) => Err(PropError::new(format!(
                "parse: Int value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for u64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(u64::from_le_bytes(bytes)),
            Err(_) => Err(PropError::new(format!(
                "parse: UInt value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for f64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(f64::from_le_bytes(bytes)),
            Err(_) => Err(PropError::new(format!(
                "parse: Float value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for Vec<u8> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        Ok(raw_attribute)
    }
}

impl AttributeValue for bool {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        if raw_attribute.len() == 1 {
            return Ok(raw_attribute[0] & 1 == 1);
        }
        Err(PropError::new(format!(
            "parse: Bool value expected to be 1 byte, but got {}",
            raw_attribute.len()
        )))
    }
}

impl AttributeValue for DateTime<FixedOffset> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, PropError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => {
                let nanos = i64::from_le_bytes(bytes);
                Ok(DateTime::from_timestamp_nanos(nanos).into())
            }
            Err(_) => Err(PropError::new(format!(
                "parse: Timestamp expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub struct Path {
    tokens: Vec<String>,
}

impl Display for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.tokens
                .iter()
                .map(|t| t.replace('.', "\\."))
                .collect::<Vec<String>>()
                .join(".")
        )
    }
}

impl Debug for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "path: {:?}", self.tokens)
    }
}

impl From<&str> for Path {
    fn from(value: &str) -> Self {
        let mut token = String::new();
        let mut tokens: Vec<String> = Vec::new();
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '.' => {
                    tokens.push(token);
                    token = String::new();
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        token.push(next);
                    }
                }
                _ => token.push(ch),
            }
        }
        tokens.push(token);

        Self { tokens }
    }
}

impl Path {
    pub fn new<T: Into<String>>(tokens: Vec<T>) -> Self {
        Self {
            tokens: tokens.into_iter().map(|i| i.into()).collect(),
        }
    }
    pub fn tokens(&self) -> Vec<&str> {
        self.tokens.iter().map(String::as_str).collect()
    }

    pub fn is_request(&self) -> bool {
        !self.tokens.is_empty() && self.tokens[0] == "request"
    }
}

#[derive(Debug)]
pub enum PropertyError {
    Get(PropError),
    Parse(PropError),
}

impl Error for PropertyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PropertyError::Get(err) => Some(err),
            PropertyError::Parse(err) => Some(err),
        }
    }
}

impl Display for PropertyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PropertyError::Get(e) => {
                write!(f, "PropertyError::Get {{ {e:?} }}")
            }
            PropertyError::Parse(e) => {
                write!(f, "PropertyError::Parse {{ {e:?} }}")
            }
        }
    }
}
