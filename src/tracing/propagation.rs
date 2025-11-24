use crate::data::Headers;
use opentelemetry::propagation::{Extractor, Injector};
use std::collections::HashMap;

pub struct HeadersExtractor {
    headers_map: HashMap<String, String>,
}

impl HeadersExtractor {
    pub fn new(headers: &Headers) -> Self {
        Self {
            headers_map: headers.clone().into(),
        }
    }
}

impl Extractor for HeadersExtractor {
    fn get(&self, key: &str) -> Option<&str> {
        self.headers_map.get(key).map(|s| s.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.headers_map.keys().map(|k| k.as_str()).collect()
    }
}

pub struct HeadersInjector<'a> {
    headers: &'a mut Vec<(String, Vec<u8>)>,
}

impl<'a> HeadersInjector<'a> {
    pub fn new(headers: &'a mut Vec<(String, Vec<u8>)>) -> Self {
        Self { headers }
    }
}

impl<'a> Injector for HeadersInjector<'a> {
    fn set(&mut self, key: &str, value: String) {
        if !value.is_empty() {
            self.headers.push((key.to_string(), value.into_bytes()));
        }
    }
}
