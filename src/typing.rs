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
