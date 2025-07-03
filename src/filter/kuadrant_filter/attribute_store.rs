use crate::data::PropertyPath;
use cel_interpreter::objects::{Key, Map};
use cel_interpreter::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
enum Token {
    Node(HashMap<String, Token>),
    Value(Value),
}

pub struct AttributeStore {
    data: HashMap<String, Token>,
}

impl AttributeStore {
    pub fn new() -> Self {
        AttributeStore {
            data: HashMap::new(),
        }
    }

    pub fn add(&mut self, path: PropertyPath, val: Value) {
        let mut node = &mut self.data;
        let mut it = path.tokens().into_iter();
        while let Some(token) = it.next() {
            if it.len() != 0 {
                node = match node
                    .entry(token.to_string())
                    .or_insert(Token::Node(HashMap::default()))
                {
                    Token::Node(node) => node,
                    // a value was installed, on this path...
                    // so that value should resolve from there on
                    Token::Value(_) => break,
                };
            } else {
                node.insert(token.to_string(), Token::Value(val.clone()));
            }
        }
    }

    pub fn contains_path(&self, path: &PropertyPath) -> bool {
        let mut node = &self.data;
        let mut it = path.tokens().into_iter();
        let mut res = false;
        while let Some(token) = it.next() {
            if it.len() != 0 {
                node = match node.get(&token.to_string()) {
                    Some(Token::Node(node)) => node,
                    Some(Token::Value(_)) => return false,
                    None => return false,
                };
            } else {
                res = match node.get(&token.to_string()) {
                    Some(Token::Node(_)) => false,
                    Some(Token::Value(_)) => true,
                    None => false,
                };
            }
        }
        res
    }

    pub fn cel_map(&self) -> Map {
        Self::cel_map_recursive(&self.data)
    }

    fn cel_map_recursive(map: &HashMap<String, Token>) -> Map {
        let mut out: HashMap<Key, Value> = HashMap::default();
        for (key, value) in map.iter() {
            let k = key.clone().into();
            let v = match value {
                Token::Value(v) => v.clone(),
                Token::Node(map) => Value::Map(Self::cel_map_recursive(map)),
            };
            out.insert(k, v);
        }
        Map { map: Arc::new(out) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add() {
        let mut store = AttributeStore::new();
        store.add("request.method".into(), "GET".into());
        store.add("request.referer".into(), "https://example.com/".into());
        // should not be added  as request.referer is endpoint
        store.add("request.referer.other".into(), "some".into());
        store.add("source.address".into(), "192.168.0.1".into());
        store.add("destination.port".into(), 443.into());

        assert_eq!(3, store.data.len());
        assert!(store.data.contains_key("source"));
        assert!(store.data.contains_key("destination"));
        assert!(store.data.contains_key("request"));

        match store.data.get("source").expect("source is some") {
            Token::Node(map) => {
                assert_eq!(map.len(), 1);
                assert!(map.get("address").is_some());

                match map.get("address").expect("address is some") {
                    Token::Node(_) => unreachable!("Not supposed to get here!"),
                    Token::Value(v) => assert_eq!(*v, Value::from("192.168.0.1")),
                }
            }
            Token::Value(_) => unreachable!("Not supposed to get here!"),
        }

        match store.data.get("destination").expect("destination is some") {
            Token::Node(map) => {
                assert_eq!(map.len(), 1);
                match map.get("port").expect("port is some") {
                    Token::Node(_) => unreachable!("Not supposed to get here!"),
                    Token::Value(v) => assert_eq!(*v, Value::from(443)),
                }
            }
            Token::Value(_) => unreachable!("Not supposed to get here!"),
        }

        match store.data.get("request").expect("request is some") {
            Token::Node(map) => {
                assert_eq!(map.len(), 2);
                assert!(map.get("method").is_some());
                match map.get("method").expect("method is some") {
                    Token::Node(_) => unreachable!("Not supposed to get here!"),
                    Token::Value(v) => assert_eq!(*v, Value::from("GET")),
                }
                assert!(map.get("referer").is_some());
                match map.get("referer").expect("referer is some") {
                    Token::Node(_) => unreachable!("Not supposed to get here!"),
                    Token::Value(v) => assert_eq!(*v, Value::from("https://example.com/")),
                }
            }
            Token::Value(_) => unreachable!("Not supposed to get here!"),
        }
    }

    #[test]
    fn cel_map() {
        let mut store = AttributeStore::new();
        store.add("request.method".into(), "GET".into());
        store.add("request.referer".into(), "https://example.com/".into());
        store.add("source.address".into(), "192.168.0.1".into());
        store.add("destination.port".into(), 443.into());

        let map = store.cel_map();

        assert!(map.get(&"source".into()).is_some());
        assert!(map.get(&"destination".into()).is_some());
        assert!(map.get(&"source".into()).is_some());

        match map.get(&"source".into()).expect("source is some") {
            Value::Map(map) => {
                assert!(map.get(&"address".into()).is_some());
                match map.get(&"address".into()).expect("address is some") {
                    Value::String(v) => assert_eq!(**v, String::from("192.168.0.1")),
                    _ => unreachable!("Not supposed to get here!"),
                }
            }
            _ => unreachable!("Not supposed to get here!"),
        }

        match map.get(&"destination".into()).expect("destination is some") {
            Value::Map(map) => {
                assert!(map.get(&"port".into()).is_some());
                match map.get(&"port".into()).expect("port is some") {
                    Value::Int(v) => assert_eq!(*v, 443),
                    _ => unreachable!("Not supposed to get here!"),
                }
            }
            _ => unreachable!("Not supposed to get here!"),
        }

        match map.get(&"request".into()).expect("request is some") {
            Value::Map(map) => {
                assert!(map.get(&"method".into()).is_some());
                match map.get(&"method".into()).expect("method is some") {
                    Value::String(v) => assert_eq!(**v, String::from("GET")),
                    _ => unreachable!("Not supposed to get here!"),
                }

                assert!(map.get(&"referer".into()).is_some());
                match map.get(&"referer".into()).expect("referer is some") {
                    Value::String(v) => assert_eq!(**v, String::from("https://example.com/")),
                    _ => unreachable!("Not supposed to get here!"),
                }
            }
            _ => unreachable!("Not supposed to get here!"),
        }
    }

    #[test]
    fn contains_path() {
        let mut store = AttributeStore::new();
        store.add("a".into(), "1".into());
        store.add("b.b1".into(), "1".into());
        store.add("b.b2".into(), "1".into());
        store.add("c.c1.c11".into(), 1.into());
        store.add("c.c2.c21".into(), 1.into());

        assert!(store.contains_path(&"a".into()));
        assert!(!store.contains_path(&"a.something".into()));
        assert!(!store.contains_path(&"b".into()));
        assert!(store.contains_path(&"b.b1".into()));
        assert!(store.contains_path(&"b.b2".into()));
        assert!(!store.contains_path(&"c.c1".into()));
        assert!(store.contains_path(&"c.c1.c11".into()));
        assert!(store.contains_path(&"c.c2.c21".into()));
        assert!(!store.contains_path(&"c.c2.c21.something".into()));
    }
}
