use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]

pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn append(&mut self, key: String, value: String) {
        self.0.push((key, value));
    }

    pub fn set(&mut self, key: String, value: String) {
        self.0.retain(|(k, _)| k != &key);
        self.0.push((key, value));
    }

    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn to_vec(&self) -> Vec<(&str, &str)> {
        self.0
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    pub fn inner(&self) -> &[(String, String)] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<(String, String)> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn extend(&mut self, other: Headers) {
        self.0.extend(other.0);
    }
}

impl Default for Headers {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<(String, String)>> for Headers {
    fn from(vec: Vec<(String, String)>) -> Self {
        Self(vec)
    }
}

impl From<Headers> for Vec<(String, String)> {
    fn from(headers: Headers) -> Self {
        headers.0
    }
}

impl From<Headers> for HashMap<String, String> {
    fn from(headers: Headers) -> Self {
        let mut map: HashMap<String, String> = HashMap::new();
        for (key, value) in headers.0 {
            map.entry(key)
                .and_modify(|existing: &mut String| {
                    existing.push(',');
                    existing.push_str(&value);
                })
                .or_insert(value);
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_append() {
        let mut headers = Headers::new();
        headers.append("X-Custom".to_string(), "value1".to_string());
        headers.append("X-Custom".to_string(), "value2".to_string());

        assert_eq!(headers.len(), 2);
        assert_eq!(headers.get_all("X-Custom"), vec!["value1", "value2"]);
    }

    #[test]
    fn test_set_replaces_existing() {
        let mut headers: Headers = vec![
            ("X-Custom".to_string(), "old1".to_string()),
            ("X-Custom".to_string(), "old2".to_string()),
            ("X-Other".to_string(), "keep".to_string()),
        ]
        .into();

        headers.set("X-Custom".to_string(), "new".to_string());

        assert_eq!(headers.len(), 2);
        assert_eq!(headers.get_all("X-Custom"), vec!["new"]);
        assert_eq!(headers.get("X-Other"), Some("keep"));
    }

    #[test]
    fn test_remove() {
        let mut headers: Headers = vec![
            ("X-Remove".to_string(), "value1".to_string()),
            ("X-Remove".to_string(), "value2".to_string()),
            ("X-Keep".to_string(), "value".to_string()),
        ]
        .into();

        headers.remove("X-Remove");

        assert_eq!(headers.len(), 1);
        assert_eq!(headers.get_all("X-Remove"), Vec::<&str>::new());
        assert_eq!(headers.get("X-Keep"), Some("value"));
    }

    #[test]
    fn test_get() {
        let headers: Headers = vec![
            ("X-First".to_string(), "value1".to_string()),
            ("X-First".to_string(), "value2".to_string()),
        ]
        .into();

        assert_eq!(headers.get("X-First"), Some("value1"));
        assert_eq!(headers.get("X-Missing"), None);
    }

    #[test]
    fn test_to_vec() {
        let headers: Headers = vec![("X-Test".to_string(), "value".to_string())].into();

        let vec = headers.to_vec();
        assert_eq!(vec, vec![("X-Test", "value")]);
    }

    #[test]
    fn test_extend() {
        let mut headers1: Headers = vec![("X-First".to_string(), "value1".to_string())].into();

        let headers2: Headers = vec![("X-Second".to_string(), "value2".to_string())].into();

        headers1.extend(headers2);

        assert_eq!(headers1.len(), 2);
        assert_eq!(headers1.get("X-First"), Some("value1"));
        assert_eq!(headers1.get("X-Second"), Some("value2"));
    }

    #[test]
    fn test_multi_value_headers() {
        let headers: Headers = vec![
            ("Set-Cookie".to_string(), "session=abc".to_string()),
            ("Set-Cookie".to_string(), "token=xyz".to_string()),
            ("Set-Cookie".to_string(), "user=123".to_string()),
        ]
        .into();

        let cookies = headers.get_all("Set-Cookie");
        assert_eq!(cookies, vec!["session=abc", "token=xyz", "user=123"]);
        assert_eq!(headers.get("Set-Cookie"), Some("session=abc"));
    }

    #[test]
    fn test_from_headers_to_hashmap_joins_duplicates() {
        let headers: Headers = vec![
            ("Accept".to_string(), "text/html".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            ("X-Single".to_string(), "value".to_string()),
        ]
        .into();

        let map: HashMap<String, String> = headers.into();

        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("Accept"),
            Some(&"text/html,application/json".to_string())
        );
        assert_eq!(map.get("X-Single"), Some(&"value".to_string()));
    }
}
