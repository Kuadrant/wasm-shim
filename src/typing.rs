use std::fmt::Write;

pub enum TypedProperty {
    String(String),
    Integer(i64),
    Bytes(Vec<u8>),
}

impl TypedProperty {
    pub fn string(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(string) => TypedProperty::String(string),
            Err(err) => TypedProperty::bytes(err.into_bytes()),
        }
    }

    pub fn integer(bytes: Vec<u8>) -> Self {
        if bytes.len() == 8 {
            TypedProperty::Integer(i64::from_le_bytes(
                bytes[..8].try_into().expect("This has to be 8 bytes long!"),
            ))
        } else {
            TypedProperty::bytes(bytes)
        }
    }

    pub fn bytes(bytes: Vec<u8>) -> Self {
        TypedProperty::Bytes(bytes.to_vec())
    }
}

impl TypedProperty {
    pub fn as_string(&self) -> String {
        match self {
            TypedProperty::String(str) => str.clone(),
            TypedProperty::Integer(int) => int.to_string(),
            TypedProperty::Bytes(bytes) => {
                let mut str = String::with_capacity(4 * bytes.len());
                for byte in bytes {
                    write!(str, "\\x{:02x}", byte).unwrap();
                }
                str
            }
        }
    }
}

impl PartialEq<str> for TypedProperty {
    fn eq(&self, other: &str) -> bool {
        match self {
            TypedProperty::String(str) => str == other,
            TypedProperty::Integer(int) => int.to_string().as_str() == other,
            TypedProperty::Bytes(bytes) => bytes.as_slice() == other.as_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::typing::TypedProperty;

    #[test]
    fn escapes_bytes() {
        let prop = TypedProperty::bytes(vec![0xb2, 0x42, 0x01]);
        assert_eq!("\\xb2\\x42\\x01", prop.as_string());
    }
}
