use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cel::Value;
use tracing::error;

use super::body_parser::{parse_json_scalar, BodyParser};
use crate::data::attribute::AttributeError;
use crate::kuadrant::context::BodyContext;

pub(crate) struct JsonBodyParser {
    fields: Vec<String>,
    parser: Option<acutejson::Parser>,
    buffers: Rc<RefCell<HashMap<String, Vec<u8>>>>,
    extracted: HashMap<String, Value>,
    bytes_consumed: usize,
    complete: bool,
}

impl JsonBodyParser {
    pub fn new(fields: Vec<String>) -> Result<Self, AttributeError> {
        let buffers: Rc<RefCell<HashMap<String, Vec<u8>>>> = Rc::new(RefCell::new(HashMap::new()));
        let results: Rc<RefCell<HashMap<String, Vec<u8>>>> = Rc::clone(&buffers);

        let mut builder = acutejson::Builder::new();
        for field in &fields {
            let field_name = field.clone();
            let field_buffers = Rc::clone(&results);
            field_buffers
                .borrow_mut()
                .insert(field_name.clone(), Vec::new());

            builder = match builder.register(field, move |bytes, is_complete| {
                let mut bufs = field_buffers.borrow_mut();
                if let Some(buf) = bufs.get_mut(&field_name) {
                    buf.extend_from_slice(bytes);

                    if is_complete && std::str::from_utf8(buf).is_err() {
                        error!("Body field value is not valid UTF-8");
                    }
                } else {
                    error!("Buffer not found for field {}", field_name);
                }
            }) {
                Ok(b) => b,
                Err(e) => {
                    error!("Invalid JSON pointer: {e:?}");
                    return Err(AttributeError::Parse(format!(
                        "Invalid JSON pointer: {e:?}"
                    )));
                }
            };
        }

        Ok(Self {
            fields,
            parser: Some(builder.build()),
            buffers,
            extracted: HashMap::new(),
            bytes_consumed: 0,
            complete: false,
        })
    }

    fn finalize_extracted(&mut self) {
        let buffers = self.buffers.borrow();
        for (field, raw_bytes) in buffers.iter() {
            if let Ok(raw_value) = std::str::from_utf8(raw_bytes) {
                if !raw_value.is_empty() {
                    let value = parse_json_scalar(raw_value);
                    self.extracted.insert(field.clone(), value);
                }
            }
        }
    }
}

impl BodyParser for JsonBodyParser {
    fn bytes_consumed(&self) -> usize {
        self.bytes_consumed
    }

    fn finalize(&mut self) -> Result<(), AttributeError> {
        if let Some(ref mut parser) = self.parser {
            parser
                .finish()
                .map_err(|e| AttributeError::Parse(format!("JSON finalize error: {e}")))?;
        }
        self.finalize_extracted();
        Ok(())
    }

    fn remaining_fields(&self) -> Vec<&String> {
        self.fields
            .iter()
            .filter(|f| !self.extracted.contains_key(f.as_str()))
            .collect()
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), AttributeError> {
        if self.complete {
            return Ok(());
        }

        let parser = match self.parser.as_mut() {
            Some(p) => p,
            None => return Err(AttributeError::Parse("Parser not initialized".to_string())),
        };

        match parser.feed(chunk) {
            Ok(acutejson::Status::Done) => {
                self.complete = true;
                self.finalize_extracted();
            }
            Ok(acutejson::Status::NeedMore) => {}
            Err(e) => {
                error!("JSON parse error: {e:?}");
                return Err(AttributeError::Parse(format!("JSON parse error: {e:?}")));
            }
        }

        self.bytes_consumed += chunk.len();
        Ok(())
    }

    fn populate(&self, body_ctx: &mut BodyContext) {
        for (field, value) in &self.extracted {
            body_ctx.set_value(field, value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn single_chunk_extracts_field() {
        let mut parser = JsonBodyParser::new(vec!["/model".to_string()]).unwrap();

        parser.feed(br#"{"model":"gpt-4"}"#).unwrap();

        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/model"),
            Some(&Value::String(Arc::new("gpt-4".to_string())))
        );
    }

    #[test]
    fn chunked_feed_extracts_field() {
        let mut parser = JsonBodyParser::new(vec!["/stream".to_string()]).unwrap();

        parser.feed(br#"{"model":"gpt"#).unwrap();
        assert_eq!(parser.remaining_fields(), vec![&"/stream".to_string()]);

        parser.feed(br#"-4","stream":true}"#).unwrap();
        assert!(parser.remaining_fields().is_empty());

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value("/stream"), Some(&Value::Bool(true)));
    }

    #[test]
    fn missing_field_remains_in_remaining() {
        let mut parser = JsonBodyParser::new(vec!["/missing".to_string()]).unwrap();

        parser.feed(br#"{"other":1}"#).unwrap();
        parser.finalize().unwrap();

        assert_eq!(parser.remaining_fields(), vec![&"/missing".to_string()]);
    }

    #[test]
    fn multiple_fields_extracted() {
        let mut parser = JsonBodyParser::new(vec!["/a".to_string(), "/b".to_string()]).unwrap();

        parser.feed(br#"{"a":10,"b":"hello"}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(body_ctx.get_value("/a"), Some(&Value::Int(10)));
        assert_eq!(
            body_ctx.get_value("/b"),
            Some(&Value::String(Arc::new("hello".to_string())))
        );
    }

    #[test]
    fn malformed_json_returns_error() {
        let mut parser = JsonBodyParser::new(vec!["/field".to_string()]).unwrap();

        assert!(parser.feed(b"{not valid json}").is_err());
    }

    #[test]
    fn finalize_catches_truncated_json() {
        let mut parser = JsonBodyParser::new(vec!["/field".to_string()]).unwrap();

        parser.feed(br#"{"field": "#).unwrap();
        assert!(parser.finalize().is_err());
    }

    #[test]
    fn invalid_json_pointer_returns_error() {
        assert!(JsonBodyParser::new(vec!["no-leading-slash".to_string()]).is_err());
    }

    #[test]
    fn nested_field_extracted() {
        let mut parser = JsonBodyParser::new(vec!["/usage/total_tokens".to_string()]).unwrap();

        parser.feed(br#"{"usage":{"total_tokens":42}}"#).unwrap();

        let mut body_ctx = BodyContext::default();
        parser.populate(&mut body_ctx);
        assert_eq!(
            body_ctx.get_value("/usage/total_tokens"),
            Some(&Value::Int(42))
        );
    }

    #[test]
    fn bytes_consumed_tracks_fed_bytes() {
        let mut parser = JsonBodyParser::new(vec!["/a".to_string()]).unwrap();
        assert_eq!(parser.bytes_consumed(), 0);

        parser.feed(br#"{"a""#).unwrap();
        assert_eq!(parser.bytes_consumed(), 4);

        parser.feed(br#":1}"#).unwrap();
        assert_eq!(parser.bytes_consumed(), 7);
    }
}
