use chrono::{DateTime, FixedOffset};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

use crate::v2::kuadrant::cache::CachedValue;

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeState<T> {
    Pending,
    Available(T),
}

impl<T> AttributeState<T> {
    pub fn map<U, F>(self, f: F) -> AttributeState<U>
    where
        F: FnOnce(T) -> U,
    {
        match self {
            AttributeState::Pending => AttributeState::Pending,
            AttributeState::Available(val) => AttributeState::Available(f(val)),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum AttributeError {
    NotAvailable(String),
    Retrieval(String),
    Parse(String),
}

impl Error for AttributeError {}

impl Display for AttributeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AttributeError::NotAvailable(msg) => {
                write!(f, "AttributeError::NotAvailable {{ {msg:?} }}")
            }
            AttributeError::Retrieval(msg) => {
                write!(f, "AttributeError::Retrieval {{ {msg:?} }}")
            }
            AttributeError::Parse(msg) => {
                write!(f, "AttributeError::Parse {{ {msg:?} }}")
            }
        }
    }
}

pub trait AttributeValue {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError>
    where
        Self: Sized;

    fn from_cached(cached: &CachedValue) -> Result<Option<Self>, AttributeError>
    where
        Self: Sized,
    {
        match cached {
            CachedValue::Bytes(Some(bytes)) => Ok(Some(Self::parse(bytes.clone())?)),
            CachedValue::Bytes(None) => Ok(None),
            CachedValue::Map(_) => Err(AttributeError::Parse(
                "Expected bytes, found map".to_string(),
            )),
        }
    }
}

impl AttributeValue for String {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        String::from_utf8(raw_attribute).map_err(|err| {
            AttributeError::Parse(format!(
                "parse: failed to parse selector String value, error: {err}"
            ))
        })
    }
}

impl AttributeValue for i64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(i64::from_le_bytes(bytes)),
            Err(_) => Err(AttributeError::Parse(format!(
                "parse: Int value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for u64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(u64::from_le_bytes(bytes)),
            Err(_) => Err(AttributeError::Parse(format!(
                "parse: UInt value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for f64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => Ok(f64::from_le_bytes(bytes)),
            Err(_) => Err(AttributeError::Parse(format!(
                "parse: Float value expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for Vec<u8> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        Ok(raw_attribute)
    }
}

impl AttributeValue for bool {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        if raw_attribute.len() == 1 {
            return Ok(raw_attribute[0] & 1 == 1);
        }
        Err(AttributeError::Parse(format!(
            "parse: Bool value expected to be 1 byte, but got {}",
            raw_attribute.len()
        )))
    }
}

impl AttributeValue for DateTime<FixedOffset> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        let ra_len = raw_attribute.len();
        match <[u8; 8]>::try_from(raw_attribute) {
            Ok(bytes) => {
                let nanos = i64::from_le_bytes(bytes);
                Ok(DateTime::from_timestamp_nanos(nanos).into())
            }
            Err(_) => Err(AttributeError::Parse(format!(
                "parse: Timestamp expected to be 8 bytes, but got {ra_len}",
            ))),
        }
    }
}

impl AttributeValue for HashMap<String, String> {
    fn parse(_raw_attribute: Vec<u8>) -> Result<Self, AttributeError> {
        Err(AttributeError::Parse(
            "Maps do not support parse".to_string(),
        ))
    }

    fn from_cached(cached: &CachedValue) -> Result<Option<Self>, AttributeError> {
        match cached {
            CachedValue::Map(map) => Ok(Some(map.clone())),
            CachedValue::Bytes(_) => Err(AttributeError::Parse(
                "Expected map, found bytes".to_string(),
            )),
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

pub fn wasm_prop(tokens: &[&str]) -> Path {
    let mut flat_attr = "filter_state.wasm\\.kuadrant\\.".to_string();
    flat_attr.push_str(tokens.join("\\.").as_str());
    flat_attr.as_str().into()
}
