use crate::configuration::Path;
use chrono::{DateTime, FixedOffset};
use proxy_wasm::hostcalls;

pub trait Attribute {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String>
    where
        Self: Sized;
}

impl Attribute for String {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        String::from_utf8(raw_attribute).map_err(|err| {
            format!(
                "parse: failed to parse selector String value, error: {}",
                err
            )
        })
    }
}

impl Attribute for i64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: Int value expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(i64::from_le_bytes(
            raw_attribute[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        ))
    }
}

impl Attribute for u64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: UInt value expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(u64::from_le_bytes(
            raw_attribute[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        ))
    }
}

impl Attribute for f64 {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: Float value expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(f64::from_le_bytes(
            raw_attribute[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        ))
    }
}

impl Attribute for Vec<u8> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        Ok(raw_attribute)
    }
}

impl Attribute for bool {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 1 {
            return Err(format!(
                "parse: Bool value expected to be 1 byte, but got {}",
                raw_attribute.len()
            ));
        }
        Ok(raw_attribute[0] & 1 == 1)
    }
}

impl Attribute for DateTime<FixedOffset> {
    fn parse(raw_attribute: Vec<u8>) -> Result<Self, String> {
        if raw_attribute.len() != 8 {
            return Err(format!(
                "parse: Timestamp expected to be 8 bytes, but got {}",
                raw_attribute.len()
            ));
        }

        let nanos = i64::from_le_bytes(
            raw_attribute.as_slice()[..8]
                .try_into()
                .expect("This has to be 8 bytes long!"),
        );
        Ok(DateTime::from_timestamp_nanos(nanos).into())
    }
}

pub fn get_attribute<T>(attr: &str) -> Result<T, String>
where
    T: Attribute,
{
    match hostcalls::get_property(Path::from(attr).tokens()).unwrap() {
        None => Err(format!("get_attribute: not found: {}", attr)),
        Some(attribute_bytes) => T::parse(attribute_bytes),
    }
}
