use crate::data::Headers;
use opentelemetry::propagation::{Extractor, Injector};

pub struct HeadersExtractor<'a> {
    headers: &'a Headers,
}

impl<'a> HeadersExtractor<'a> {
    pub fn new(headers: &'a Headers) -> Self {
        Self { headers }
    }
}

impl<'a> Extractor for HeadersExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.headers.get(key)
    }

    fn keys(&self) -> Vec<&str> {
        self.headers
            .inner()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect()
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
        self.headers.push((key.to_string(), value.into_bytes()));
    }
}
