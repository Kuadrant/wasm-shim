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

    fn is_complete(&self) -> bool {
        self.complete
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
