use crate::typing::TypedProperty;
use std::collections::BTreeMap;

pub struct EnvoyTypeMapper {
    known_properties: BTreeMap<String, Box<dyn Fn(Vec<u8>) -> TypedProperty>>,
}

impl EnvoyTypeMapper {
    pub fn new() -> Self {
        let mut properties: BTreeMap<String, Box<dyn Fn(Vec<u8>) -> TypedProperty>> = BTreeMap::new();
        properties.insert("foo.bar".to_string(), Box::new(TypedProperty::string));
        properties.insert("foo.car".to_string(), Box::new(TypedProperty::integer));
        Self {
            known_properties: properties,
        }
    }

    pub fn typed(&self, path: &str, raw: Vec<u8>) -> Option<TypedProperty> {
        self.known_properties.get(path).map(|mapper| mapper(raw))
    }
}

// trait TypedEvaluator<T> {
//     fn eval(&self, operator: &WhenConditionOperator, operand: &T);
// }
//
// impl TypedEvaluator<str> for TypedProperty {
//     fn eval(&self, operator: &WhenConditionOperator, operand: &str) {
//         match *operator {
//             WhenConditionOperator::Equal => {}
//             WhenConditionOperator::NotEqual => {}
//             WhenConditionOperator::StartsWith => {}
//             WhenConditionOperator::EndsWith => {}
//             WhenConditionOperator::Matches => {}
//         }
//     }
// }
