use cel::common::types::*;
use cel::objects::Key;
use cel::{StructDef, Value};
use prost_reflect::Cardinality;
use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind as ProtoKind, MapKey, MessageDescriptor, ReflectMessage,
    Value as ProtoValue,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

#[derive(Debug)]
pub enum ConversionError {
    TypeMismatch {
        field: String,
        expected: String,
        got: String,
    },
    UnsupportedFieldType {
        field: String,
        kind: String,
    },
    NotAStruct,
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConversionError::TypeMismatch {
                field,
                expected,
                got,
            } => write!(
                f,
                "Type mismatch for field '{}': expected {}, got {}",
                field, expected, got
            ),
            ConversionError::UnsupportedFieldType { field, kind } => {
                write!(f, "Unsupported field type for '{}': {}", field, kind)
            }
            ConversionError::NotAStruct => write!(f, "CEL value is not a struct"),
        }
    }
}

impl std::error::Error for ConversionError {}

fn is_map_field(field: &FieldDescriptor) -> bool {
    if field.cardinality() != Cardinality::Repeated {
        return false;
    }
    if let ProtoKind::Message(msg_desc) = field.kind() {
        msg_desc.is_map_entry()
    } else {
        false
    }
}

/// Converts protobuf MessageDescriptors to CEL StructDefs
pub struct DescriptorConverter;

impl DescriptorConverter {
    /// Returns a vector of [`StructDef`]s for the given [`MessageDescriptor`], including all nested message types.
    pub fn collect_struct_defs(
        descriptor: &MessageDescriptor,
    ) -> Result<Vec<StructDef>, ConversionError> {
        let mut defs = Vec::new();
        let mut to_register = vec![descriptor.clone()];
        let mut visited = HashSet::new();

        while let Some(desc) = to_register.pop() {
            if !visited.insert(desc.full_name().to_string()) {
                continue;
            }

            for field in desc.fields() {
                if let ProtoKind::Message(nested_desc) = field.kind() {
                    to_register.push(nested_desc);
                }
            }

            if desc.is_map_entry() {
                continue;
            }

            let struct_def = Self::to_struct_def(&desc)?;
            defs.push(struct_def);
        }

        Ok(defs)
    }

    /// Convert a protobuf MessageDescriptor to a CEL StructDef
    pub fn to_struct_def(descriptor: &MessageDescriptor) -> Result<StructDef, ConversionError> {
        let mut struct_def = StructDef::new(descriptor.full_name().to_string());

        for field in descriptor.fields() {
            let cel_type = if is_map_field(&field) {
                MAP_TYPE
            } else if field.cardinality() == Cardinality::Repeated {
                LIST_TYPE
            } else {
                Self::protobuf_kind_to_cel_type(field.kind())?
            };

            struct_def = struct_def.add_field(field.name().to_string(), cel_type);
        }

        Ok(struct_def)
    }

    fn protobuf_kind_to_cel_type(
        kind: ProtoKind,
    ) -> Result<cel::common::types::Type, ConversionError> {
        match kind {
            ProtoKind::Bool => Ok(BOOL_TYPE),
            ProtoKind::Int32
            | ProtoKind::Int64
            | ProtoKind::Sint32
            | ProtoKind::Sint64
            | ProtoKind::Sfixed32
            | ProtoKind::Sfixed64 => Ok(INT_TYPE),
            ProtoKind::Uint32 | ProtoKind::Uint64 | ProtoKind::Fixed32 | ProtoKind::Fixed64 => {
                Ok(UINT_TYPE)
            }
            ProtoKind::Float | ProtoKind::Double => Ok(DOUBLE_TYPE),
            ProtoKind::String => Ok(STRING_TYPE),
            ProtoKind::Bytes => Ok(BYTES_TYPE),
            ProtoKind::Message(desc) => {
                if desc.full_name() == "google.protobuf.Timestamp" {
                    Ok(TIMESTAMP_TYPE)
                } else {
                    Ok(Type::new_struct(desc.full_name().to_string()))
                }
            }
            ProtoKind::Enum(_) => {
                // Enums are represented as INT in CEL
                Ok(INT_TYPE)
            }
        }
    }
}

pub fn deny_response_struct_def() -> StructDef {
    StructDef::new("DenyResponse".to_string())
        .add_field("status".to_string(), UINT_TYPE)
        .add_field_with_default("body".to_string(), Box::new(CelString::from("")))
        .add_field_with_default("headers".to_string(), Box::new(CelList::from(vec![])))
}

pub fn cel_value_to_header_pairs(value: &Value) -> Vec<(String, String)> {
    let Value::List(items) = value else {
        return vec![];
    };

    let mut pairs = Vec::new();
    for item in items.iter() {
        match item {
            Value::List(inner) if inner.len() == 2 => {
                if let (Value::String(k), Value::String(v)) = (&inner[0], &inner[1]) {
                    pairs.push((k.to_string(), v.to_string()));
                }
            }
            Value::Struct(s) => {
                if let (Some(key_val), Some(value_val)) =
                    (s.field_value("key"), s.field_value("value"))
                {
                    if let (Some(k), Some(v)) = (
                        key_val.downcast_ref::<CelString>(),
                        value_val.downcast_ref::<CelString>(),
                    ) {
                        pairs.push((k.inner().to_string(), v.inner().to_string()));
                        continue;
                    }
                }

                if let Some(header_val) = s.field_value("header") {
                    if let Some(header_struct) = header_val.downcast_ref::<CelStruct>() {
                        if let (Some(key_val), Some(value_val)) = (
                            header_struct.field_value("key"),
                            header_struct.field_value("value"),
                        ) {
                            if let (Some(k), Some(v)) = (
                                key_val.downcast_ref::<CelString>(),
                                value_val.downcast_ref::<CelString>(),
                            ) {
                                pairs.push((k.inner().to_string(), v.inner().to_string()));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pairs
}

pub struct MessageConverter;

impl MessageConverter {
    pub fn cel_to_dynamic_message(
        cel_value: &Value,
        descriptor: &MessageDescriptor,
    ) -> Result<DynamicMessage, ConversionError> {
        match cel_value {
            Value::Struct(cel_struct) => Self::struct_to_dynamic_message(cel_struct, descriptor),
            _ => Err(ConversionError::NotAStruct),
        }
    }

    pub fn cel_value_to_bytes(value: &Value) -> Result<Vec<u8>, ConversionError> {
        use prost::Message;
        match value {
            Value::Map(_) => Ok(Self::cel_to_prost_struct(value)?.encode_to_vec()),
            _ => Ok(Self::cel_to_prost_value(value)?.encode_to_vec()),
        }
    }

    fn cel_to_prost_value(value: &Value) -> Result<prost_types::Value, ConversionError> {
        let kind = match value {
            Value::Null => prost_types::value::Kind::NullValue(0),
            Value::Float(f) => prost_types::value::Kind::NumberValue(*f),
            Value::Int(i) => prost_types::value::Kind::NumberValue(*i as f64),
            Value::UInt(u) => prost_types::value::Kind::NumberValue(*u as f64),
            Value::String(s) => prost_types::value::Kind::StringValue(s.to_string()),
            Value::Bool(b) => prost_types::value::Kind::BoolValue(*b),
            Value::Bytes(b) => {
                prost_types::value::Kind::StringValue(String::from_utf8_lossy(b).to_string())
            }
            Value::Map(_) => {
                prost_types::value::Kind::StructValue(Self::cel_to_prost_struct(value)?)
            }
            Value::List(items) => {
                let values: Result<Vec<_>, _> =
                    items.iter().map(Self::cel_to_prost_value).collect();
                prost_types::value::Kind::ListValue(prost_types::ListValue { values: values? })
            }
            _ => {
                return Err(ConversionError::TypeMismatch {
                    field: "value".to_string(),
                    expected: "null, number, string, bool, map, or list".to_string(),
                    got: format!("{:?}", value),
                })
            }
        };
        Ok(prost_types::Value { kind: Some(kind) })
    }

    fn cel_to_prost_struct(value: &Value) -> Result<prost_types::Struct, ConversionError> {
        use std::collections::BTreeMap;
        match value {
            Value::Map(m) => {
                let mut fields = BTreeMap::new();
                for (k, v) in m.map.iter() {
                    let key_str = match k {
                        Key::String(s) => s.to_string(),
                        _ => {
                            return Err(ConversionError::TypeMismatch {
                                field: "map key".to_string(),
                                expected: "string".to_string(),
                                got: format!("{:?}", k),
                            })
                        }
                    };
                    fields.insert(key_str, Self::cel_to_prost_value(v)?);
                }
                Ok(prost_types::Struct { fields })
            }
            _ => Err(ConversionError::TypeMismatch {
                field: "value".to_string(),
                expected: "map".to_string(),
                got: format!("{:?}", value),
            }),
        }
    }

    pub fn dynamic_message_to_cel(message: &DynamicMessage) -> Result<Value, ConversionError> {
        let descriptor = message.descriptor();
        let mut cel_struct = CelStruct::new(descriptor.full_name().to_string());

        for field in descriptor.fields() {
            if !message.has_field(&field) && field.supports_presence() {
                continue;
            }
            let field_value = message.get_field(&field);
            let cel_value = Self::proto_value_to_cel_val(field_value.as_ref())?;
            cel_struct.add_field_value(field.name().to_string(), Cow::Owned(cel_value));
        }

        Ok(Value::Struct(Arc::new(cel_struct)))
    }

    fn proto_value_to_cel_val(
        proto_value: &ProtoValue,
    ) -> Result<Box<dyn cel::common::value::Val>, ConversionError> {
        match proto_value {
            ProtoValue::Bool(b) => Ok(Box::new(CelBool::from(*b))),
            ProtoValue::I32(i) => Ok(Box::new(CelInt::from(*i as i64))),
            ProtoValue::I64(i) => Ok(Box::new(CelInt::from(*i))),
            ProtoValue::U32(u) => Ok(Box::new(CelUInt::from(*u as u64))),
            ProtoValue::U64(u) => Ok(Box::new(CelUInt::from(*u))),
            ProtoValue::F32(f) => Ok(Box::new(CelDouble::from(*f as f64))),
            ProtoValue::F64(f) => Ok(Box::new(CelDouble::from(*f))),
            ProtoValue::String(s) => Ok(Box::new(CelString::from(s.clone()))),
            ProtoValue::Bytes(b) => Ok(Box::new(CelBytes::from(b.to_vec()))),
            ProtoValue::EnumNumber(n) => Ok(Box::new(CelInt::from(*n as i64))),
            ProtoValue::Message(m) => {
                let descriptor = m.descriptor();

                // Special handling for google.protobuf.Timestamp ->  Value::Timestamp
                if descriptor.full_name() == "google.protobuf.Timestamp" {
                    return Self::proto_timestamp_to_cel_timestamp(m);
                }

                if descriptor.full_name() == "google.protobuf.Struct" {
                    return Self::unwrap_proto_struct(m);
                }

                if descriptor.full_name() == "google.protobuf.Value" {
                    return Self::unwrap_proto_value(m);
                }

                if descriptor.full_name() == "google.protobuf.ListValue" {
                    return Self::unwrap_proto_list_value(m);
                }

                let mut cel_struct = CelStruct::new(descriptor.full_name().to_string());

                for field in descriptor.fields() {
                    if !m.has_field(&field) && field.supports_presence() {
                        continue;
                    }
                    let field_value = m.get_field(&field);
                    let cel_value = Self::proto_value_to_cel_val(field_value.as_ref())?;
                    cel_struct.add_field_value(field.name().to_string(), Cow::Owned(cel_value));
                }

                Ok(Box::new(cel_struct))
            }
            ProtoValue::List(items) => {
                let cel_items: Result<Vec<_>, _> = items
                    .iter()
                    .map(|item| Self::proto_value_to_cel_val(item))
                    .collect();
                Ok(Box::new(CelList::from(cel_items?)))
            }
            ProtoValue::Map(entries) => {
                let mut map = HashMap::new();
                for (key, value) in entries.iter() {
                    let cel_key = Self::map_key_to_cel(key);
                    let cel_value = Self::proto_value_to_cel_val(value)?;
                    map.insert(cel_key, cel_value);
                }
                Ok(Box::new(CelMap::from(map)))
            }
        }
    }

    fn map_key_to_cel(key: &MapKey) -> cel::common::types::CelMapKey {
        match key {
            MapKey::Bool(b) => CelMapKey::Bool(CelBool::from(*b)),
            MapKey::I32(i) => CelMapKey::Int(CelInt::from(*i as i64)),
            MapKey::I64(i) => CelMapKey::Int(CelInt::from(*i)),
            MapKey::U32(u) => CelMapKey::UInt(CelUInt::from(*u as u64)),
            MapKey::U64(u) => CelMapKey::UInt(CelUInt::from(*u)),
            MapKey::String(s) => CelMapKey::String(CelString::from(s.clone())),
        }
    }

    fn struct_to_dynamic_message(
        cel_struct: &Arc<CelStruct>,
        descriptor: &MessageDescriptor,
    ) -> Result<DynamicMessage, ConversionError> {
        let mut message = DynamicMessage::new(descriptor.clone());

        for field in descriptor.fields() {
            if let Some(val) = cel_struct.field_value(field.name()) {
                let proto_value = Self::cel_val_to_proto_value(val, &field)?;
                message.set_field(&field, proto_value);
            }
        }

        Ok(message)
    }

    fn cel_val_to_proto_value(
        cel_val: &dyn cel::common::value::Val,
        field: &FieldDescriptor,
    ) -> Result<ProtoValue, ConversionError> {
        if is_map_field(field) {
            Self::cel_map_to_proto_map(cel_val, field)
        } else if field.cardinality() == Cardinality::Repeated {
            Self::cel_list_to_proto_list(cel_val, field)
        } else {
            Self::cel_val_to_single_proto_value(cel_val, field)
        }
    }

    fn cel_list_to_proto_list(
        cel_val: &dyn cel::common::value::Val,
        field: &FieldDescriptor,
    ) -> Result<ProtoValue, ConversionError> {
        let field_name = field.name();

        let list =
            cel_val
                .downcast_ref::<CelList>()
                .ok_or_else(|| ConversionError::TypeMismatch {
                    field: field_name.to_string(),
                    expected: "list".to_string(),
                    got: format!("{:?}", cel_val),
                })?;

        let mut proto_values = Vec::new();
        for item in list.iter() {
            // Process each list element as a single (non-repeated) value
            let element_value = Self::cel_val_to_single_proto_value(item.as_ref(), field)?;
            proto_values.push(element_value);
        }

        Ok(ProtoValue::List(proto_values))
    }

    fn cel_map_to_proto_map(
        cel_val: &dyn cel::common::value::Val,
        field: &FieldDescriptor,
    ) -> Result<ProtoValue, ConversionError> {
        let field_name = field.name();

        let cel_map = cel_val
            .downcast_ref::<cel::common::types::CelMap>()
            .ok_or_else(|| ConversionError::TypeMismatch {
                field: field_name.to_string(),
                expected: "map".to_string(),
                got: format!("{:?}", cel_val),
            })?;

        let map_entry_desc = if let ProtoKind::Message(desc) = field.kind() {
            desc
        } else {
            return Err(ConversionError::UnsupportedFieldType {
                field: field_name.to_string(),
                kind: format!("{:?}", field.kind()),
            });
        };

        let key_field = map_entry_desc.get_field_by_name("key").ok_or_else(|| {
            ConversionError::UnsupportedFieldType {
                field: field_name.to_string(),
                kind: "map entry missing 'key' field".to_string(),
            }
        })?;

        let value_field = map_entry_desc.get_field_by_name("value").ok_or_else(|| {
            ConversionError::UnsupportedFieldType {
                field: field_name.to_string(),
                kind: "map entry missing 'value' field".to_string(),
            }
        })?;

        let mut proto_map = HashMap::new();
        for (cel_key, cel_value) in cel_map.iter() {
            let proto_key = Self::cel_map_key_to_proto(cel_key, &key_field)?;
            let proto_value =
                Self::cel_val_to_single_proto_value(cel_value.as_ref(), &value_field)?;
            proto_map.insert(proto_key, proto_value);
        }

        Ok(ProtoValue::Map(proto_map))
    }

    fn cel_map_key_to_proto(
        cel_key: &cel::common::types::CelMapKey,
        key_field: &FieldDescriptor,
    ) -> Result<MapKey, ConversionError> {
        use cel::common::types::CelMapKey;

        match cel_key {
            CelMapKey::Bool(b) => Ok(MapKey::Bool(*b.inner())),
            CelMapKey::Int(i) => {
                let value = *i.inner();
                match key_field.kind() {
                    ProtoKind::Int32 | ProtoKind::Sint32 | ProtoKind::Sfixed32 => {
                        Ok(MapKey::I32(value as i32))
                    }
                    ProtoKind::Int64 | ProtoKind::Sint64 | ProtoKind::Sfixed64 => {
                        Ok(MapKey::I64(value))
                    }
                    _ => Err(ConversionError::TypeMismatch {
                        field: key_field.name().to_string(),
                        expected: "int32 or int64".to_string(),
                        got: format!("{:?}", key_field.kind()),
                    }),
                }
            }
            CelMapKey::UInt(u) => {
                let value = *u.inner();
                match key_field.kind() {
                    ProtoKind::Uint32 | ProtoKind::Fixed32 => Ok(MapKey::U32(value as u32)),
                    ProtoKind::Uint64 | ProtoKind::Fixed64 => Ok(MapKey::U64(value)),
                    _ => Err(ConversionError::TypeMismatch {
                        field: key_field.name().to_string(),
                        expected: "uint32 or uint64".to_string(),
                        got: format!("{:?}", key_field.kind()),
                    }),
                }
            }
            CelMapKey::String(s) => Ok(MapKey::String(s.inner().to_string())),
        }
    }

    fn cel_val_to_single_proto_value(
        cel_val: &dyn cel::common::value::Val,
        field: &FieldDescriptor,
    ) -> Result<ProtoValue, ConversionError> {
        let field_name = field.name();

        match field.kind() {
            ProtoKind::Bool => {
                let b = cel_val.downcast_ref::<CelBool>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "bool".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::Bool(*b.inner()))
            }
            ProtoKind::Int32 | ProtoKind::Sint32 | ProtoKind::Sfixed32 => {
                let i = cel_val.downcast_ref::<CelInt>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "int".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::I32(*i.inner() as i32))
            }
            ProtoKind::Int64 | ProtoKind::Sint64 | ProtoKind::Sfixed64 => {
                let i = cel_val.downcast_ref::<CelInt>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "int".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::I64(*i.inner()))
            }
            ProtoKind::Uint32 | ProtoKind::Fixed32 => {
                let u = cel_val.downcast_ref::<CelUInt>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "uint".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::U32(*u.inner() as u32))
            }
            ProtoKind::Uint64 | ProtoKind::Fixed64 => {
                let u = cel_val.downcast_ref::<CelUInt>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "uint".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::U64(*u.inner()))
            }
            ProtoKind::Float => {
                let f = cel_val.downcast_ref::<CelDouble>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "float".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                let f64_value = *f.inner();
                if !f64_value.is_finite()
                    || f64_value < f32::MIN as f64
                    || f64_value > f32::MAX as f64
                {
                    return Err(ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "float value within f32 range".to_string(),
                        got: format!("{}", f64_value),
                    });
                }
                Ok(ProtoValue::F32(f64_value as f32))
            }
            ProtoKind::Double => {
                let f = cel_val.downcast_ref::<CelDouble>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "double".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::F64(*f.inner()))
            }
            ProtoKind::String => {
                let s = cel_val.downcast_ref::<CelString>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "string".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::String(s.inner().to_string()))
            }
            ProtoKind::Bytes => {
                let b = cel_val.downcast_ref::<CelBytes>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "bytes".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                Ok(ProtoValue::Bytes(prost::bytes::Bytes::from(
                    b.inner().to_vec(),
                )))
            }
            ProtoKind::Enum(enum_desc) => {
                let i = cel_val.downcast_ref::<CelInt>().ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "int (enum)".to_string(),
                        got: format!("{:?}", cel_val),
                    }
                })?;
                let i64_value = *i.inner();
                if i64_value < i32::MIN as i64 || i64_value > i32::MAX as i64 {
                    return Err(ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: "int32 value for enum".to_string(),
                        got: format!("{}", i64_value),
                    });
                }
                let value = enum_desc.get_value(i64_value as i32).ok_or_else(|| {
                    ConversionError::TypeMismatch {
                        field: field_name.to_string(),
                        expected: format!("valid enum value for {}", enum_desc.name()),
                        got: format!("{}", i64_value),
                    }
                })?;
                Ok(ProtoValue::EnumNumber(value.number()))
            }
            ProtoKind::Message(nested_desc) => {
                // Special handling for Value::Timestamp -> google.protobuf.Timestamp
                if nested_desc.full_name() == "google.protobuf.Timestamp" {
                    if let Some(cel_ts) = cel_val.downcast_ref::<CelTimestamp>() {
                        return Self::cel_timestamp_to_proto_message(
                            cel_ts,
                            &nested_desc,
                            field_name,
                        );
                    }
                }

                let nested_message =
                    Self::struct_from_val_to_message(cel_val, &nested_desc, field_name)?;
                Ok(ProtoValue::Message(nested_message))
            }
        }
    }

    fn cel_timestamp_to_proto_message(
        cel_ts: &CelTimestamp,
        descriptor: &MessageDescriptor,
        field_name: &str,
    ) -> Result<ProtoValue, ConversionError> {
        let dt = cel_ts.inner();
        let mut message = DynamicMessage::new(descriptor.clone());

        let seconds_field = descriptor.get_field_by_name("seconds").ok_or_else(|| {
            ConversionError::TypeMismatch {
                field: field_name.to_string(),
                expected: "Timestamp with seconds field".to_string(),
                got: "missing seconds field".to_string(),
            }
        })?;
        let nanos_field =
            descriptor
                .get_field_by_name("nanos")
                .ok_or_else(|| ConversionError::TypeMismatch {
                    field: field_name.to_string(),
                    expected: "Timestamp with nanos field".to_string(),
                    got: "missing nanos field".to_string(),
                })?;

        message.set_field(&seconds_field, ProtoValue::I64(dt.timestamp()));
        message.set_field(
            &nanos_field,
            ProtoValue::I32(dt.timestamp_subsec_nanos() as i32),
        );

        Ok(ProtoValue::Message(message))
    }

    fn proto_timestamp_to_cel_timestamp(
        message: &DynamicMessage,
    ) -> Result<Box<dyn cel::common::value::Val>, ConversionError> {
        use chrono::{DateTime, FixedOffset};

        let descriptor = message.descriptor();

        let seconds_field = descriptor.get_field_by_name("seconds").ok_or_else(|| {
            ConversionError::TypeMismatch {
                field: "seconds".to_string(),
                expected: "google.protobuf.Timestamp must have seconds field".to_string(),
                got: "field not found".to_string(),
            }
        })?;
        let nanos_field =
            descriptor
                .get_field_by_name("nanos")
                .ok_or_else(|| ConversionError::TypeMismatch {
                    field: "nanos".to_string(),
                    expected: "google.protobuf.Timestamp must have nanos field".to_string(),
                    got: "field not found".to_string(),
                })?;

        let seconds = message.get_field(&seconds_field);
        let nanos = message.get_field(&nanos_field);

        let seconds_value = match seconds.as_ref() {
            ProtoValue::I64(s) => *s,
            _ => {
                return Err(ConversionError::TypeMismatch {
                    field: "seconds".to_string(),
                    expected: "i64".to_string(),
                    got: format!("{:?}", seconds),
                })
            }
        };

        let nanos_value = match nanos.as_ref() {
            ProtoValue::I32(n) => *n as u32,
            _ => {
                return Err(ConversionError::TypeMismatch {
                    field: "nanos".to_string(),
                    expected: "i32".to_string(),
                    got: format!("{:?}", nanos),
                })
            }
        };

        let dt: DateTime<FixedOffset> = DateTime::from_timestamp(seconds_value, nanos_value)
            .ok_or_else(|| ConversionError::TypeMismatch {
                field: "timestamp".to_string(),
                expected: "valid timestamp".to_string(),
                got: format!("seconds={}, nanos={}", seconds_value, nanos_value),
            })?
            .into();

        Ok(Box::new(CelTimestamp::from(dt)))
    }

    fn unwrap_proto_struct(
        message: &DynamicMessage,
    ) -> Result<Box<dyn cel::common::value::Val>, ConversionError> {
        let descriptor = message.descriptor();
        let fields_field = descriptor.get_field_by_name("fields").ok_or_else(|| {
            ConversionError::TypeMismatch {
                field: "fields".to_string(),
                expected: "google.protobuf.Struct must have fields".to_string(),
                got: "field not found".to_string(),
            }
        })?;

        let fields_value = message.get_field(&fields_field);
        match fields_value.as_ref() {
            ProtoValue::Map(entries) => {
                let mut map = HashMap::new();
                for (key, value) in entries.iter() {
                    let cel_key: CelMapKey = Self::map_key_to_cel(key);
                    let cel_value = Self::proto_value_to_cel_val(value)?;
                    map.insert(cel_key, cel_value);
                }
                Ok(Box::new(CelMap::from(map)))
            }
            _ => Err(ConversionError::TypeMismatch {
                field: "fields".to_string(),
                expected: "map".to_string(),
                got: format!("{:?}", fields_value),
            }),
        }
    }

    fn unwrap_proto_value(
        message: &DynamicMessage,
    ) -> Result<Box<dyn cel::common::value::Val>, ConversionError> {
        let descriptor = message.descriptor();

        for field in descriptor.fields() {
            if field.containing_oneof().is_some() && !message.has_field(&field) {
                continue;
            }

            let field_value = message.get_field(&field);
            return match field.name() {
                "null_value" => Ok(Box::new(CelNull)),
                "number_value" => Self::proto_value_to_cel_val(field_value.as_ref()),
                "string_value" => Self::proto_value_to_cel_val(field_value.as_ref()),
                "bool_value" => Self::proto_value_to_cel_val(field_value.as_ref()),
                "struct_value" => Self::proto_value_to_cel_val(field_value.as_ref()),
                "list_value" => Self::proto_value_to_cel_val(field_value.as_ref()),
                _ => Err(ConversionError::UnsupportedFieldType {
                    field: field.name().to_string(),
                    kind: "unknown google.protobuf.Value variant".to_string(),
                }),
            };
        }

        Ok(Box::new(CelNull))
    }

    fn unwrap_proto_list_value(
        message: &DynamicMessage,
    ) -> Result<Box<dyn cel::common::value::Val>, ConversionError> {
        let descriptor = message.descriptor();
        let values_field = descriptor.get_field_by_name("values").ok_or_else(|| {
            ConversionError::TypeMismatch {
                field: "values".to_string(),
                expected: "google.protobuf.ListValue must have values".to_string(),
                got: "field not found".to_string(),
            }
        })?;

        let values = message.get_field(&values_field);
        match values.as_ref() {
            ProtoValue::List(items) => {
                let cel_items: Result<Vec<_>, _> = items
                    .iter()
                    .map(|item| Self::proto_value_to_cel_val(item))
                    .collect();
                Ok(Box::new(CelList::from(cel_items?)))
            }
            _ => Err(ConversionError::TypeMismatch {
                field: "values".to_string(),
                expected: "list".to_string(),
                got: format!("{:?}", values),
            }),
        }
    }

    fn struct_from_val_to_message(
        cel_val: &dyn cel::common::value::Val,
        descriptor: &MessageDescriptor,
        field_name: &str,
    ) -> Result<DynamicMessage, ConversionError> {
        let nested_struct =
            cel_val
                .downcast_ref::<CelStruct>()
                .ok_or_else(|| ConversionError::TypeMismatch {
                    field: field_name.to_string(),
                    expected: "struct (nested message)".to_string(),
                    got: format!("{:?}", cel_val),
                })?;

        let mut message = DynamicMessage::new(descriptor.clone());

        for field in descriptor.fields() {
            if let Some(val) = nested_struct.field_value(field.name()) {
                let proto_value = Self::cel_val_to_proto_value(val, &field)?;
                message.set_field(&field, proto_value);
            }
        }

        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cel::common::value::Val;
    use cel::{Context, Env, Program};
    use prost::Message;
    use prost_types::{field_descriptor_proto, DescriptorProto, FieldDescriptorProto};
    use prost_types::{FileDescriptorProto, FileDescriptorSet, OneofDescriptorProto};
    use std::sync::Arc;

    fn create_test_message_descriptor() -> MessageDescriptor {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("TestMessage".to_string()),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("string_field".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("int32_field".to_string()),
                        number: Some(2),
                        r#type: Some(field_descriptor_proto::Type::Int32.into()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("bool_field".to_string()),
                        number: Some(3),
                        r#type: Some(field_descriptor_proto::Type::Bool.into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        pool.get_message_by_name("test.TestMessage")
            .expect("Message not found")
    }

    #[test]
    fn test_descriptor_to_struct_def() {
        let descriptor = create_test_message_descriptor();
        let struct_def = DescriptorConverter::to_struct_def(&descriptor);

        assert!(struct_def.is_ok());
        let struct_def = struct_def.unwrap();

        // Verify we can register it with CEL
        let mut env = cel::Env::stdlib();
        env.add_struct(struct_def);

        // Should be able to compile an expression using the struct
        let result = Program::compile(
            "test.TestMessage { string_field: 'hello', int32_field: 42, bool_field: true }",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_cel_to_dynamic_message() {
        let descriptor = create_test_message_descriptor();
        let struct_def =
            DescriptorConverter::to_struct_def(&descriptor).expect("Failed to convert descriptor");

        let mut env = cel::Env::stdlib();
        env.add_struct(struct_def);

        let ctx = Context::with_env(Arc::new(env));

        let program = Program::compile(
            "test.TestMessage { string_field: 'hello', int32_field: 42, bool_field: true }",
        )
        .expect("Failed to compile");

        let cel_value = program.execute(&ctx).expect("Failed to execute");

        let message = MessageConverter::cel_to_dynamic_message(&cel_value, &descriptor)
            .expect("Failed to convert");

        // Verify fields
        let string_field = message
            .get_field_by_name("string_field")
            .expect("string_field not found");
        assert_eq!(
            string_field.as_ref(),
            &ProtoValue::String("hello".to_string())
        );

        let int_field = message
            .get_field_by_name("int32_field")
            .expect("int32_field not found");
        assert_eq!(int_field.as_ref(), &ProtoValue::I32(42));

        let bool_field = message
            .get_field_by_name("bool_field")
            .expect("bool_field not found");
        assert_eq!(bool_field.as_ref(), &ProtoValue::Bool(true));
    }

    #[test]
    fn test_nested_messages() {
        // Create a descriptor with nested messages
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![
                DescriptorProto {
                    name: Some("Inner".to_string()),
                    field: vec![FieldDescriptorProto {
                        name: Some("value".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("Outer".to_string()),
                    field: vec![
                        FieldDescriptorProto {
                            name: Some("name".to_string()),
                            number: Some(1),
                            r#type: Some(field_descriptor_proto::Type::String.into()),
                            ..Default::default()
                        },
                        FieldDescriptorProto {
                            name: Some("inner".to_string()),
                            number: Some(2),
                            r#type: Some(field_descriptor_proto::Type::Message.into()),
                            type_name: Some(".test.Inner".to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let outer_descriptor = pool
            .get_message_by_name("test.Outer")
            .expect("Outer message not found");

        // Register all message types
        let mut env = cel::Env::stdlib();
        for def in DescriptorConverter::collect_struct_defs(&outer_descriptor)
            .expect("Failed to collect struct defs")
        {
            env.add_struct(def);
        }

        let ctx = Context::with_env(Arc::new(env));

        // Build a CEL expression with nested message
        let program = Program::compile(
            r#"test.Outer { name: "parent", inner: test.Inner { value: "child" } }"#,
        )
        .expect("Failed to compile");

        let cel_value = program.execute(&ctx).expect("Failed to execute");

        // Convert to DynamicMessage
        let message = MessageConverter::cel_to_dynamic_message(&cel_value, &outer_descriptor)
            .expect("Failed to convert");

        // Verify outer fields
        let name_field = message
            .get_field_by_name("name")
            .expect("name field not found");
        assert_eq!(
            name_field.as_ref(),
            &ProtoValue::String("parent".to_string())
        );

        // Verify nested message
        let inner_field = message
            .get_field_by_name("inner")
            .expect("inner field not found");
        assert!(matches!(inner_field.as_ref(), ProtoValue::Message(_)));

        if let ProtoValue::Message(inner_msg) = inner_field.as_ref() {
            let value_field = inner_msg
                .get_field_by_name("value")
                .expect("value field not found");
            assert_eq!(
                value_field.as_ref(),
                &ProtoValue::String("child".to_string())
            );
        }
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let descriptor = create_test_message_descriptor();
        let struct_def =
            DescriptorConverter::to_struct_def(&descriptor).expect("Failed to convert descriptor");

        let mut env = cel::Env::stdlib();
        env.add_struct(struct_def);

        let ctx = Context::with_env(Arc::new(env));

        let program = Program::compile(
            "test.TestMessage { string_field: 'test', int32_field: 123, bool_field: false }",
        )
        .expect("Failed to compile");

        let cel_value = program.execute(&ctx).expect("Failed to execute");

        let message = MessageConverter::cel_to_dynamic_message(&cel_value, &descriptor)
            .expect("Failed to convert");

        // Encode to protobuf bytes
        let mut bytes = Vec::new();
        message.encode(&mut bytes).expect("Failed to encode");

        // Decode back
        let decoded =
            DynamicMessage::decode(descriptor, bytes.as_slice()).expect("Failed to decode");

        // Verify fields match
        assert_eq!(
            message.get_field_by_name("string_field"),
            decoded.get_field_by_name("string_field")
        );
        assert_eq!(
            message.get_field_by_name("int32_field"),
            decoded.get_field_by_name("int32_field")
        );
        assert_eq!(
            message.get_field_by_name("bool_field"),
            decoded.get_field_by_name("bool_field")
        );
    }

    #[test]
    fn test_dynamic_message_to_cel() {
        let descriptor = create_test_message_descriptor();
        let mut message = DynamicMessage::new(descriptor.clone());

        message.set_field_by_name("string_field", ProtoValue::String("hello".to_string()));
        message.set_field_by_name("int32_field", ProtoValue::I32(42));
        message.set_field_by_name("bool_field", ProtoValue::Bool(true));

        let cel_value =
            MessageConverter::dynamic_message_to_cel(&message).expect("Failed to convert to CEL");

        assert!(matches!(&cel_value, Value::Struct(_)));

        if let Value::Struct(s) = &cel_value {
            assert_eq!(s.name(), "test.TestMessage");

            let string_val = s
                .field_value("string_field")
                .expect("string_field not found");
            assert_eq!(
                string_val.downcast_ref::<CelString>().map(|v| v.inner()),
                Some("hello")
            );

            let int_val = s.field_value("int32_field").expect("int32_field not found");
            assert_eq!(
                int_val.downcast_ref::<CelInt>().map(|v| *v.inner()),
                Some(42)
            );

            let bool_val = s.field_value("bool_field").expect("bool_field not found");
            assert_eq!(
                bool_val.downcast_ref::<CelBool>().map(|v| *v.inner()),
                Some(true)
            );
        }
    }

    #[test]
    fn test_cel_to_message_to_cel_roundtrip() {
        let descriptor = create_test_message_descriptor();
        let struct_def =
            DescriptorConverter::to_struct_def(&descriptor).expect("Failed to convert descriptor");

        let mut env = cel::Env::stdlib();
        env.add_struct(struct_def);

        let ctx = Context::with_env(Arc::new(env));

        let program = Program::compile(
            "test.TestMessage { string_field: 'roundtrip', int32_field: 999, bool_field: true }",
        )
        .expect("Failed to compile");

        let original_cel = program.execute(&ctx).expect("Failed to execute");

        let message = MessageConverter::cel_to_dynamic_message(&original_cel, &descriptor)
            .expect("Failed to convert to message");

        let converted_cel =
            MessageConverter::dynamic_message_to_cel(&message).expect("Failed to convert to CEL");

        assert!(matches!(&converted_cel, Value::Struct(_)));

        if let Value::Struct(s) = &converted_cel {
            assert_eq!(s.name(), "test.TestMessage");

            let string_val = s
                .field_value("string_field")
                .expect("string_field not found");
            assert_eq!(
                string_val.downcast_ref::<CelString>().map(|v| v.inner()),
                Some("roundtrip")
            );

            let int_val = s.field_value("int32_field").expect("int32_field not found");
            assert_eq!(
                int_val.downcast_ref::<CelInt>().map(|v| *v.inner()),
                Some(999)
            );

            let bool_val = s.field_value("bool_field").expect("bool_field not found");
            assert_eq!(
                bool_val.downcast_ref::<CelBool>().map(|v| *v.inner()),
                Some(true)
            );
        }
    }

    #[test]
    fn test_nested_message_to_cel() {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![
                DescriptorProto {
                    name: Some("Inner".to_string()),
                    field: vec![FieldDescriptorProto {
                        name: Some("value".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("Outer".to_string()),
                    field: vec![
                        FieldDescriptorProto {
                            name: Some("name".to_string()),
                            number: Some(1),
                            r#type: Some(field_descriptor_proto::Type::String.into()),
                            ..Default::default()
                        },
                        FieldDescriptorProto {
                            name: Some("inner".to_string()),
                            number: Some(2),
                            r#type: Some(field_descriptor_proto::Type::Message.into()),
                            type_name: Some(".test.Inner".to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let outer_descriptor = pool
            .get_message_by_name("test.Outer")
            .expect("Outer not found");
        let inner_descriptor = pool
            .get_message_by_name("test.Inner")
            .expect("Inner not found");

        let mut inner_message = DynamicMessage::new(inner_descriptor);
        inner_message.set_field_by_name("value", ProtoValue::String("nested".to_string()));

        let mut outer_message = DynamicMessage::new(outer_descriptor);
        outer_message.set_field_by_name("name", ProtoValue::String("parent".to_string()));
        outer_message.set_field_by_name("inner", ProtoValue::Message(inner_message));

        let cel_value = MessageConverter::dynamic_message_to_cel(&outer_message)
            .expect("Failed to convert to CEL");

        assert!(matches!(&cel_value, Value::Struct(_)));

        if let Value::Struct(outer_struct) = &cel_value {
            assert_eq!(outer_struct.name(), "test.Outer");

            let name_val = outer_struct.field_value("name").expect("name not found");
            assert_eq!(
                name_val.downcast_ref::<CelString>().map(|v| v.inner()),
                Some("parent")
            );

            let inner_val = outer_struct.field_value("inner").expect("inner not found");
            let inner_struct = inner_val
                .downcast_ref::<CelStruct>()
                .expect("inner should be a struct");
            assert_eq!(inner_struct.name(), "test.Inner");

            let value_val = inner_struct.field_value("value").expect("value not found");
            assert_eq!(
                value_val.downcast_ref::<CelString>().map(|v| v.inner()),
                Some("nested")
            );
        }
    }

    #[test]
    fn test_map_field_to_cel() {
        use std::collections::HashMap;

        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("MapMessage".to_string()),
                field: vec![FieldDescriptorProto {
                    name: Some("tags".to_string()),
                    number: Some(1),
                    r#type: Some(field_descriptor_proto::Type::Message.into()),
                    type_name: Some(".test.MapMessage.TagsEntry".to_string()),
                    label: Some(prost_types::field_descriptor_proto::Label::Repeated.into()),
                    ..Default::default()
                }],
                nested_type: vec![DescriptorProto {
                    name: Some("TagsEntry".to_string()),
                    options: Some(prost_types::MessageOptions {
                        map_entry: Some(true),
                        ..Default::default()
                    }),
                    field: vec![
                        FieldDescriptorProto {
                            name: Some("key".to_string()),
                            number: Some(1),
                            r#type: Some(field_descriptor_proto::Type::String.into()),
                            ..Default::default()
                        },
                        FieldDescriptorProto {
                            name: Some("value".to_string()),
                            number: Some(2),
                            r#type: Some(field_descriptor_proto::Type::String.into()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let descriptor = pool
            .get_message_by_name("test.MapMessage")
            .expect("MapMessage not found");

        let mut message = DynamicMessage::new(descriptor);

        let mut map_entries = HashMap::new();
        map_entries.insert(
            MapKey::String("env".to_string()),
            ProtoValue::String("prod".to_string()),
        );
        map_entries.insert(
            MapKey::String("region".to_string()),
            ProtoValue::String("us-east-1".to_string()),
        );

        message.set_field_by_name("tags", ProtoValue::Map(map_entries));

        let cel_value =
            MessageConverter::dynamic_message_to_cel(&message).expect("Failed to convert to CEL");

        assert!(matches!(&cel_value, Value::Struct(_)));

        if let Value::Struct(s) = &cel_value {
            let tags_val = s.field_value("tags").expect("tags not found");
            let map = tags_val
                .downcast_ref::<cel::common::types::CelMap>()
                .expect("tags should be a map");

            assert_eq!(map.len(), 2);
        }
    }

    #[test]
    fn test_cel_to_map_field() {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("MapMessage".to_string()),
                field: vec![FieldDescriptorProto {
                    name: Some("labels".to_string()),
                    number: Some(1),
                    r#type: Some(field_descriptor_proto::Type::Message.into()),
                    type_name: Some(".test.MapMessage.LabelsEntry".to_string()),
                    label: Some(prost_types::field_descriptor_proto::Label::Repeated.into()),
                    ..Default::default()
                }],
                nested_type: vec![DescriptorProto {
                    name: Some("LabelsEntry".to_string()),
                    options: Some(prost_types::MessageOptions {
                        map_entry: Some(true),
                        ..Default::default()
                    }),
                    field: vec![
                        FieldDescriptorProto {
                            name: Some("key".to_string()),
                            number: Some(1),
                            r#type: Some(field_descriptor_proto::Type::String.into()),
                            ..Default::default()
                        },
                        FieldDescriptorProto {
                            name: Some("value".to_string()),
                            number: Some(2),
                            r#type: Some(field_descriptor_proto::Type::String.into()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let descriptor = pool
            .get_message_by_name("test.MapMessage")
            .expect("MapMessage not found");

        let mut env = cel::Env::stdlib();
        for def in DescriptorConverter::collect_struct_defs(&descriptor)
            .expect("Failed to collect struct defs")
        {
            env.add_struct(def);
        }

        let ctx = Context::with_env(Arc::new(env));

        let program = Program::compile(
            r#"test.MapMessage { labels: {"env": "prod", "region": "us-east-1"} }"#,
        )
        .expect("Failed to compile");

        let cel_value = program.execute(&ctx).expect("Failed to execute");

        let message = MessageConverter::cel_to_dynamic_message(&cel_value, &descriptor)
            .expect("Failed to convert");

        let labels_field = message
            .get_field_by_name("labels")
            .expect("labels field not found");

        assert!(matches!(labels_field.as_ref(), ProtoValue::Map(_)));

        if let ProtoValue::Map(map) = labels_field.as_ref() {
            assert_eq!(map.len(), 2);
            assert_eq!(
                map.get(&MapKey::String("env".to_string())),
                Some(&ProtoValue::String("prod".to_string()))
            );
            assert_eq!(
                map.get(&MapKey::String("region".to_string())),
                Some(&ProtoValue::String("us-east-1".to_string()))
            );
        }
    }

    #[test]
    fn test_list_field_to_cel() {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("ListMessage".to_string()),
                field: vec![FieldDescriptorProto {
                    name: Some("items".to_string()),
                    number: Some(1),
                    r#type: Some(field_descriptor_proto::Type::String.into()),
                    label: Some(prost_types::field_descriptor_proto::Label::Repeated.into()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let descriptor = pool
            .get_message_by_name("test.ListMessage")
            .expect("ListMessage not found");

        let mut message = DynamicMessage::new(descriptor);
        message.set_field_by_name(
            "items",
            ProtoValue::List(vec![
                ProtoValue::String("first".to_string()),
                ProtoValue::String("second".to_string()),
                ProtoValue::String("third".to_string()),
            ]),
        );

        let cel_value =
            MessageConverter::dynamic_message_to_cel(&message).expect("Failed to convert to CEL");

        assert!(matches!(&cel_value, Value::Struct(_)));

        if let Value::Struct(s) = &cel_value {
            let items_val = s.field_value("items").expect("items not found");
            let list = items_val
                .downcast_ref::<cel::common::types::CelList>()
                .expect("items should be a list");

            assert_eq!(list.len(), 3);
            assert_eq!(
                list.first()
                    .and_then(|v| v.downcast_ref::<CelString>())
                    .map(|s| s.inner()),
                Some("first")
            );
            assert_eq!(
                list.get(1)
                    .and_then(|v| v.downcast_ref::<CelString>())
                    .map(|s| s.inner()),
                Some("second")
            );
            assert_eq!(
                list.get(2)
                    .and_then(|v| v.downcast_ref::<CelString>())
                    .map(|s| s.inner()),
                Some("third")
            );
        }
    }

    #[test]
    fn deny_response_struct_def_registers_and_evaluates() {
        let mut env = cel::Env::stdlib();
        env.add_struct(deny_response_struct_def());
        let ctx = Context::with_env(Arc::new(env));

        let program = Program::compile("DenyResponse{status: 429u}").expect("Failed to compile");
        let result = program.execute(&ctx).expect("Failed to execute");

        assert!(matches!(&result, Value::Struct(_)));

        if let Value::Struct(s) = &result {
            assert_eq!(s.name(), "DenyResponse");

            let status = s.field_value("status").expect("status field missing");
            assert_eq!(
                status.downcast_ref::<CelUInt>().map(|v| *v.inner()),
                Some(429)
            );

            let body = s.field_value("body").expect("body field missing");
            assert_eq!(
                body.downcast_ref::<CelString>().map(|v| v.inner()),
                Some("")
            );

            let headers = s.field_value("headers").expect("headers field missing");
            let list = headers
                .downcast_ref::<CelList>()
                .expect("headers should be a list");
            assert_eq!(list.len(), 0);
        }
    }

    #[test]
    fn deny_response_with_all_fields() {
        let mut env = cel::Env::stdlib();
        env.add_struct(deny_response_struct_def());
        let ctx = Context::with_env(Arc::new(env));

        let program =
            Program::compile("DenyResponse{status: 403u, body: 'Forbidden', headers: []}")
                .expect("Failed to compile");
        let result = program.execute(&ctx).expect("Failed to execute");

        assert!(matches!(&result, Value::Struct(_)));

        if let Value::Struct(s) = &result {
            let status = s.field_value("status").expect("status missing");
            assert_eq!(
                status.downcast_ref::<CelUInt>().map(|v| *v.inner()),
                Some(403)
            );

            let body = s.field_value("body").expect("body missing");
            assert_eq!(
                body.downcast_ref::<CelString>().map(|v| v.inner()),
                Some("Forbidden")
            );
        }
    }

    #[test]
    fn cel_value_to_header_pairs_direct_key_value() {
        let mut s1 = CelStruct::new("HeaderValue".to_string());
        s1.add_field_value(
            "key".to_string(),
            Cow::Owned(Box::new(CelString::from("x-ratelimit-limit")) as Box<dyn Val>),
        );
        s1.add_field_value(
            "value".to_string(),
            Cow::Owned(Box::new(CelString::from("100")) as Box<dyn Val>),
        );

        let mut s2 = CelStruct::new("HeaderValue".to_string());
        s2.add_field_value(
            "key".to_string(),
            Cow::Owned(Box::new(CelString::from("x-ratelimit-remaining")) as Box<dyn Val>),
        );
        s2.add_field_value(
            "value".to_string(),
            Cow::Owned(Box::new(CelString::from("42")) as Box<dyn Val>),
        );

        let list = Value::List(Arc::new(vec![
            Value::Struct(Arc::new(s1)),
            Value::Struct(Arc::new(s2)),
        ]));

        let pairs = cel_value_to_header_pairs(&list);
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            pairs[0],
            ("x-ratelimit-limit".to_string(), "100".to_string())
        );
        assert_eq!(
            pairs[1],
            ("x-ratelimit-remaining".to_string(), "42".to_string())
        );
    }

    #[test]
    fn cel_value_to_header_pairs_nested_header_field() {
        let mut header = CelStruct::new("HeaderValue".to_string());
        header.add_field_value(
            "key".to_string(),
            Cow::Owned(Box::new(CelString::from("x-custom")) as Box<dyn Val>),
        );
        header.add_field_value(
            "value".to_string(),
            Cow::Owned(Box::new(CelString::from("test")) as Box<dyn Val>),
        );

        let mut option = CelStruct::new("HeaderValueOption".to_string());
        option.add_field_value(
            "header".to_string(),
            Cow::Owned(Box::new(header) as Box<dyn Val>),
        );

        let list = Value::List(Arc::new(vec![Value::Struct(Arc::new(option))]));

        let pairs = cel_value_to_header_pairs(&list);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], ("x-custom".to_string(), "test".to_string()));
    }

    #[test]
    fn cel_value_to_header_pairs_empty_list() {
        let list = Value::List(Arc::new(vec![]));
        let pairs = cel_value_to_header_pairs(&list);
        assert!(pairs.is_empty());
    }

    #[test]
    fn cel_value_to_header_pairs_non_list_returns_empty() {
        let value = Value::String(Arc::new("not a list".to_string()));
        let pairs = cel_value_to_header_pairs(&value);
        assert!(pairs.is_empty());
    }

    #[test]
    fn cel_value_to_header_pairs_list_of_lists() {
        let list = Value::List(Arc::new(vec![
            Value::List(Arc::new(vec![
                Value::String(Arc::new("x-result".to_string())),
                Value::String(Arc::new("check-passed".to_string())),
            ])),
            Value::List(Arc::new(vec![
                Value::String(Arc::new("x-custom".to_string())),
                Value::String(Arc::new("static-value".to_string())),
            ])),
        ]));

        let pairs = cel_value_to_header_pairs(&list);
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            pairs[0],
            ("x-result".to_string(), "check-passed".to_string())
        );
        assert_eq!(
            pairs[1],
            ("x-custom".to_string(), "static-value".to_string())
        );
    }

    #[test]
    fn cel_value_to_header_pairs_list_of_lists_wrong_length_skipped() {
        let list = Value::List(Arc::new(vec![
            Value::List(Arc::new(vec![
                Value::String(Arc::new("x-valid".to_string())),
                Value::String(Arc::new("value".to_string())),
            ])),
            Value::List(Arc::new(vec![Value::String(Arc::new(
                "only-one".to_string(),
            ))])),
            Value::List(Arc::new(vec![
                Value::String(Arc::new("a".to_string())),
                Value::String(Arc::new("b".to_string())),
                Value::String(Arc::new("c".to_string())),
            ])),
        ]));

        let pairs = cel_value_to_header_pairs(&list);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], ("x-valid".to_string(), "value".to_string()));
    }

    #[test]
    fn cel_value_to_header_pairs_list_of_lists_non_string_skipped() {
        let list = Value::List(Arc::new(vec![Value::List(Arc::new(vec![
            Value::Int(42),
            Value::String(Arc::new("value".to_string())),
        ]))]));

        let pairs = cel_value_to_header_pairs(&list);
        assert!(pairs.is_empty());
    }

    #[test]
    fn test_oneof_has_checks() {
        let file_descriptor = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("TestOneofMessage".to_string()),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("variant_a".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        oneof_index: Some(0),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("variant_b".to_string()),
                        number: Some(2),
                        r#type: Some(field_descriptor_proto::Type::Int32.into()),
                        oneof_index: Some(0),
                        ..Default::default()
                    },
                ],
                oneof_decl: vec![OneofDescriptorProto {
                    name: Some("test_oneof".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create descriptor pool");

        let descriptor = pool
            .get_message_by_name("test.TestOneofMessage")
            .expect("Message not found");

        let mut message = DynamicMessage::new(descriptor.clone());
        message.set_field_by_name("variant_a", ProtoValue::String("selected".to_string()));

        let cel_value =
            MessageConverter::dynamic_message_to_cel(&message).expect("Failed to convert to CEL");

        let struct_def =
            DescriptorConverter::to_struct_def(&descriptor).expect("Failed to convert descriptor");

        let mut env = Env::stdlib();
        env.add_struct(struct_def);

        let mut ctx = Context::with_env(Arc::new(env));
        ctx.add_variable_from_value("msg", cel_value);

        let has_variant_a = Program::compile("has(msg.variant_a)")
            .expect("Failed to compile")
            .execute(&ctx)
            .expect("Failed to execute");
        assert!(
            matches!(has_variant_a, Value::Bool(true)),
            "has(msg.variant_a) should return true when variant_a is set"
        );

        let has_variant_b = Program::compile("has(msg.variant_b)")
            .expect("Failed to compile")
            .execute(&ctx)
            .expect("Failed to execute");
        assert!(
            matches!(has_variant_b, Value::Bool(false)),
            "has(msg.variant_b) should return false when variant_b is not set (unset oneof variant)"
        );
    }

    #[test]
    fn cel_timestamp_to_protobuf_timestamp_via_cel_expression() {
        use chrono::{DateTime, FixedOffset};

        let timestamp_proto = FileDescriptorProto {
            name: Some("google/protobuf/timestamp.proto".to_string()),
            package: Some("google.protobuf".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("Timestamp".to_string()),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("seconds".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::Int64.into()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("nanos".to_string()),
                        number: Some(2),
                        r#type: Some(field_descriptor_proto::Type::Int32.into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let request_proto = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            dependency: vec!["google/protobuf/timestamp.proto".to_string()],
            message_type: vec![DescriptorProto {
                name: Some("Request".to_string()),
                field: vec![FieldDescriptorProto {
                    name: Some("time".to_string()),
                    number: Some(1),
                    r#type: Some(field_descriptor_proto::Type::Message.into()),
                    type_name: Some(".google.protobuf.Timestamp".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![timestamp_proto, request_proto],
        };
        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create pool");

        let timestamp_desc = pool
            .get_message_by_name("google.protobuf.Timestamp")
            .expect("Failed to get Timestamp descriptor");
        let request_desc = pool
            .get_message_by_name("test.Request")
            .expect("Failed to get Request descriptor");

        let mut env = Env::default();
        for def in DescriptorConverter::collect_struct_defs(&timestamp_desc)
            .expect("Failed to collect struct defs")
        {
            env.add_struct(def);
        }
        for def in DescriptorConverter::collect_struct_defs(&request_desc)
            .expect("Failed to collect struct defs")
        {
            env.add_struct(def);
        }

        // Create a CEL timestamp: 2024-05-16 12:00:00 UTC (1715875200 seconds, 123456789 nanos)
        let dt: DateTime<FixedOffset> = DateTime::from_timestamp(1715875200, 123456789)
            .expect("Valid timestamp")
            .into();
        let cel_timestamp = Value::Timestamp(dt);

        let ctx = Context::with_env(Arc::new(env));
        let mut ctx_with_var = ctx;
        ctx_with_var
            .add_variable("request_time", cel_timestamp)
            .expect("Failed to add variable");

        let program = Program::compile("test.Request { time: request_time }")
            .expect("Failed to compile CEL expression");

        let cel_result = program
            .execute(&ctx_with_var)
            .expect("Failed to execute CEL expression");

        let message = MessageConverter::cel_to_dynamic_message(&cel_result, &request_desc)
            .expect("Failed to convert to DynamicMessage");

        let time_field = request_desc
            .get_field_by_name("time")
            .expect("time field not found");
        let time_value = message.get_field(&time_field);

        let seconds_field = timestamp_desc
            .get_field_by_name("seconds")
            .expect("seconds field not found");
        let nanos_field = timestamp_desc
            .get_field_by_name("nanos")
            .expect("nanos field not found");

        assert!(
            matches!(time_value.as_ref(), ProtoValue::Message(ts) if
                matches!(ts.get_field(&seconds_field).as_ref(), ProtoValue::I64(1715875200)) &&
                matches!(ts.get_field(&nanos_field).as_ref(), ProtoValue::I32(123456789))
            ),
            "Expected Message with correct timestamp values, got {:?}",
            time_value
        );
    }

    #[test]
    fn protobuf_timestamp_to_cel_timestamp_roundtrip() {
        let file_descriptor = FileDescriptorProto {
            name: Some("google/protobuf/timestamp.proto".to_string()),
            package: Some("google.protobuf".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("Timestamp".to_string()),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("seconds".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::Int64.into()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("nanos".to_string()),
                        number: Some(2),
                        r#type: Some(field_descriptor_proto::Type::Int32.into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor.clone()],
        };
        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create pool");
        let timestamp_desc = pool
            .get_message_by_name("google.protobuf.Timestamp")
            .expect("Failed to get Timestamp descriptor");

        let mut ts_msg = DynamicMessage::new(timestamp_desc.clone());
        ts_msg.set_field_by_name("seconds", ProtoValue::I64(1715875200));
        ts_msg.set_field_by_name("nanos", ProtoValue::I32(123456789));

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(ts_msg))
            .expect("Failed to convert to CEL");

        let cel_timestamp = cel_val
            .downcast_ref::<CelTimestamp>()
            .expect("Expected CelTimestamp");

        let dt = cel_timestamp.inner();
        assert_eq!(dt.timestamp(), 1715875200);
        assert_eq!(dt.timestamp_subsec_nanos(), 123456789);

        let time_field = FieldDescriptorProto {
            name: Some("time".to_string()),
            number: Some(1),
            r#type: Some(field_descriptor_proto::Type::Message.into()),
            type_name: Some(".google.protobuf.Timestamp".to_string()),
            ..Default::default()
        };

        let parent_desc = FileDescriptorProto {
            name: Some("test.proto".to_string()),
            package: Some("test".to_string()),
            dependency: vec!["google/protobuf/timestamp.proto".to_string()],
            message_type: vec![DescriptorProto {
                name: Some("TestMessage".to_string()),
                field: vec![time_field],
                ..Default::default()
            }],
            ..Default::default()
        };

        let fds_with_test = FileDescriptorSet {
            file: vec![file_descriptor, parent_desc],
        };
        let test_pool = prost_reflect::DescriptorPool::from_file_descriptor_set(fds_with_test)
            .expect("Failed to create pool");

        let test_desc = test_pool
            .get_message_by_name("test.TestMessage")
            .expect("Failed to get test descriptor");
        let field = test_desc
            .get_field_by_name("time")
            .expect("time field not found");

        let proto_value = MessageConverter::cel_val_to_proto_value(&*cel_val, &field)
            .expect("Failed to convert back to protobuf");

        let seconds_field = timestamp_desc
            .get_field_by_name("seconds")
            .expect("seconds field not found");
        let nanos_field = timestamp_desc
            .get_field_by_name("nanos")
            .expect("nanos field not found");

        assert!(
            matches!(proto_value, ProtoValue::Message(ref ts) if
                matches!(ts.get_field(&seconds_field).as_ref(), ProtoValue::I64(1715875200)) &&
                matches!(ts.get_field(&nanos_field).as_ref(), ProtoValue::I32(123456789))
            ),
            "Expected Message with correct timestamp values, got {:?}",
            proto_value
        );
    }

    fn create_well_known_types_pool() -> prost_reflect::DescriptorPool {
        let null_enum = prost_types::EnumDescriptorProto {
            name: Some("NullValue".to_string()),
            value: vec![prost_types::EnumValueDescriptorProto {
                name: Some("NULL_VALUE".to_string()),
                number: Some(0),
                ..Default::default()
            }],
            ..Default::default()
        };

        let value_msg = DescriptorProto {
            name: Some("Value".to_string()),
            field: vec![
                FieldDescriptorProto {
                    name: Some("null_value".to_string()),
                    number: Some(1),
                    r#type: Some(field_descriptor_proto::Type::Enum.into()),
                    type_name: Some(".google.protobuf.NullValue".to_string()),
                    oneof_index: Some(0),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("number_value".to_string()),
                    number: Some(2),
                    r#type: Some(field_descriptor_proto::Type::Double.into()),
                    oneof_index: Some(0),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("string_value".to_string()),
                    number: Some(3),
                    r#type: Some(field_descriptor_proto::Type::String.into()),
                    oneof_index: Some(0),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("bool_value".to_string()),
                    number: Some(4),
                    r#type: Some(field_descriptor_proto::Type::Bool.into()),
                    oneof_index: Some(0),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("struct_value".to_string()),
                    number: Some(5),
                    r#type: Some(field_descriptor_proto::Type::Message.into()),
                    type_name: Some(".google.protobuf.Struct".to_string()),
                    oneof_index: Some(0),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("list_value".to_string()),
                    number: Some(6),
                    r#type: Some(field_descriptor_proto::Type::Message.into()),
                    type_name: Some(".google.protobuf.ListValue".to_string()),
                    oneof_index: Some(0),
                    ..Default::default()
                },
            ],
            oneof_decl: vec![OneofDescriptorProto {
                name: Some("kind".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let struct_msg = DescriptorProto {
            name: Some("Struct".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("fields".to_string()),
                number: Some(1),
                r#type: Some(field_descriptor_proto::Type::Message.into()),
                type_name: Some(".google.protobuf.Struct.FieldsEntry".to_string()),
                label: Some(prost_types::field_descriptor_proto::Label::Repeated.into()),
                ..Default::default()
            }],
            nested_type: vec![DescriptorProto {
                name: Some("FieldsEntry".to_string()),
                options: Some(prost_types::MessageOptions {
                    map_entry: Some(true),
                    ..Default::default()
                }),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("key".to_string()),
                        number: Some(1),
                        r#type: Some(field_descriptor_proto::Type::String.into()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("value".to_string()),
                        number: Some(2),
                        r#type: Some(field_descriptor_proto::Type::Message.into()),
                        type_name: Some(".google.protobuf.Value".to_string()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let list_value_msg = DescriptorProto {
            name: Some("ListValue".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("values".to_string()),
                number: Some(1),
                r#type: Some(field_descriptor_proto::Type::Message.into()),
                type_name: Some(".google.protobuf.Value".to_string()),
                label: Some(prost_types::field_descriptor_proto::Label::Repeated.into()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let file_descriptor = FileDescriptorProto {
            name: Some("google/protobuf/struct.proto".to_string()),
            package: Some("google.protobuf".to_string()),
            message_type: vec![struct_msg, value_msg, list_value_msg],
            enum_type: vec![null_enum],
            ..Default::default()
        };

        let fds = FileDescriptorSet {
            file: vec![file_descriptor],
        };

        prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
            .expect("Failed to create well-known types pool")
    }

    fn make_string_value(pool: &prost_reflect::DescriptorPool, s: &str) -> DynamicMessage {
        let value_desc = pool
            .get_message_by_name("google.protobuf.Value")
            .expect("Value not found");
        let mut msg = DynamicMessage::new(value_desc);
        msg.set_field_by_name("string_value", ProtoValue::String(s.to_string()));
        msg
    }

    fn make_number_value(pool: &prost_reflect::DescriptorPool, n: f64) -> DynamicMessage {
        let value_desc = pool
            .get_message_by_name("google.protobuf.Value")
            .expect("Value not found");
        let mut msg = DynamicMessage::new(value_desc);
        msg.set_field_by_name("number_value", ProtoValue::F64(n));
        msg
    }

    fn make_struct(
        pool: &prost_reflect::DescriptorPool,
        fields: HashMap<String, DynamicMessage>,
    ) -> DynamicMessage {
        let struct_desc = pool
            .get_message_by_name("google.protobuf.Struct")
            .expect("Struct not found");
        let mut msg = DynamicMessage::new(struct_desc);
        let map: HashMap<MapKey, ProtoValue> = fields
            .into_iter()
            .map(|(k, v)| (MapKey::String(k), ProtoValue::Message(v)))
            .collect();
        msg.set_field_by_name("fields", ProtoValue::Map(map));
        msg
    }

    #[test]
    fn test_protobuf_struct_unwraps_to_map() {
        let pool = create_well_known_types_pool();

        let mut fields = HashMap::new();
        fields.insert("name".to_string(), make_string_value(&pool, "alice"));
        fields.insert("age".to_string(), make_number_value(&pool, 30.0));
        let struct_msg = make_struct(&pool, fields);

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(struct_msg))
            .expect("Failed to convert");

        let map = cel_val
            .downcast_ref::<CelMap>()
            .expect("Expected CelMap, got something else");

        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_protobuf_nested_struct_unwraps() {
        let pool = create_well_known_types_pool();

        let mut identity_fields = HashMap::new();
        identity_fields.insert("userid".to_string(), make_string_value(&pool, "alice"));

        let value_desc = pool
            .get_message_by_name("google.protobuf.Value")
            .expect("Value not found");
        let mut identity_value = DynamicMessage::new(value_desc);
        identity_value.set_field_by_name(
            "struct_value",
            ProtoValue::Message(make_struct(&pool, identity_fields)),
        );

        let mut outer_fields = HashMap::new();
        outer_fields.insert("identity".to_string(), identity_value);
        let struct_msg = make_struct(&pool, outer_fields);

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(struct_msg))
            .expect("Failed to convert");

        let outer_map = cel_val
            .downcast_ref::<CelMap>()
            .expect("Expected outer CelMap");
        assert_eq!(outer_map.len(), 1);

        let identity_val = outer_map
            .get(&CelMapKey::String(CelString::from("identity")))
            .expect("identity key not found");
        let inner_map = identity_val
            .downcast_ref::<CelMap>()
            .expect("Expected inner CelMap for identity");

        let userid_val = inner_map
            .get(&CelMapKey::String(CelString::from("userid")))
            .expect("userid key not found");
        assert_eq!(
            userid_val.downcast_ref::<CelString>().map(|s| s.inner()),
            Some("alice")
        );
    }

    #[test]
    fn test_protobuf_value_string_variant() {
        let pool = create_well_known_types_pool();
        let msg = make_string_value(&pool, "hello");

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(msg))
            .expect("Failed to convert");

        assert_eq!(
            cel_val.downcast_ref::<CelString>().map(|s| s.inner()),
            Some("hello")
        );
    }

    #[test]
    fn test_protobuf_value_number_variant() {
        let pool = create_well_known_types_pool();
        let msg = make_number_value(&pool, 42.5);

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(msg))
            .expect("Failed to convert");

        assert_eq!(
            cel_val.downcast_ref::<CelDouble>().map(|d| *d.inner()),
            Some(42.5)
        );
    }

    #[test]
    fn test_protobuf_value_bool_variant() {
        let pool = create_well_known_types_pool();
        let value_desc = pool
            .get_message_by_name("google.protobuf.Value")
            .expect("Value not found");
        let mut msg = DynamicMessage::new(value_desc);
        msg.set_field_by_name("bool_value", ProtoValue::Bool(true));

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(msg))
            .expect("Failed to convert");

        assert_eq!(
            cel_val.downcast_ref::<CelBool>().map(|b| *b.inner()),
            Some(true)
        );
    }

    #[test]
    fn test_protobuf_value_null_variant() {
        let pool = create_well_known_types_pool();
        let value_desc = pool
            .get_message_by_name("google.protobuf.Value")
            .expect("Value not found");
        let mut msg = DynamicMessage::new(value_desc);
        msg.set_field_by_name("null_value", ProtoValue::EnumNumber(0));

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(msg))
            .expect("Failed to convert");

        assert!(
            cel_val.downcast_ref::<CelNull>().is_some(),
            "Expected CelNull"
        );
    }

    #[test]
    fn test_protobuf_list_value_unwraps() {
        let pool = create_well_known_types_pool();
        let list_desc = pool
            .get_message_by_name("google.protobuf.ListValue")
            .expect("ListValue not found");

        let mut list_msg = DynamicMessage::new(list_desc);
        list_msg.set_field_by_name(
            "values",
            ProtoValue::List(vec![
                ProtoValue::Message(make_string_value(&pool, "first")),
                ProtoValue::Message(make_number_value(&pool, 2.0)),
            ]),
        );

        let cel_val = MessageConverter::proto_value_to_cel_val(&ProtoValue::Message(list_msg))
            .expect("Failed to convert");

        let list = cel_val.downcast_ref::<CelList>().expect("Expected CelList");
        assert_eq!(list.len(), 2);

        assert_eq!(
            list.first()
                .and_then(|v| v.downcast_ref::<CelString>())
                .map(|s| s.inner()),
            Some("first")
        );
        assert_eq!(
            list.get(1)
                .and_then(|v| v.downcast_ref::<CelDouble>())
                .map(|d| *d.inner()),
            Some(2.0)
        );
    }
}
