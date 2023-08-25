use crate::typing::TypedProperty;
use std::collections::BTreeMap;

type MapperFn = dyn Fn(Vec<u8>) -> TypedProperty;

pub struct EnvoyTypeMapper {
    known_properties: BTreeMap<String, Box<MapperFn>>,
}

impl EnvoyTypeMapper {
    pub fn new() -> Self {
        let mut properties: BTreeMap<String, Box<MapperFn>> = BTreeMap::new();
        properties.insert("foo.bar".to_string(), Box::new(TypedProperty::string));
        properties.insert("foo.car".to_string(), Box::new(TypedProperty::integer));
        Self {
            known_properties: properties,
        }
    }

    pub fn typed(&self, path: &str, raw: Vec<u8>) -> Result<TypedProperty, Vec<u8>> {
        match self.known_properties.get(path) {
            None => Err(raw),
            Some(mapper) => Ok(mapper(raw)),
        }
    }
}
