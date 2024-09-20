use radix_trie::Trie;

use crate::policy::Policy;

pub struct PolicyIndex {
    raw_tree: Trie<String, Policy>,
}

impl PolicyIndex {
    pub fn new() -> Self {
        Self {
            raw_tree: Trie::new(),
        }
    }

    pub fn insert(&mut self, subdomain: &str, policy: Policy) {
        let rev = Self::reverse_subdomain(subdomain);
        self.raw_tree.insert(rev, policy);
    }

    pub fn get_longest_match_policy(&self, subdomain: &str) -> Option<&Policy> {
        let rev = Self::reverse_subdomain(subdomain);
        self.raw_tree.get_ancestor_value(&rev)
    }

    fn reverse_subdomain(subdomain: &str) -> String {
        let mut s = subdomain.to_string();
        s.push('.');
        if s.starts_with('*') {
            s.remove(0);
        } else {
            s.insert(0, '$'); // $ is not a valid domain char
        }
        s.chars().rev().collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::policy::Policy;
    use crate::policy_index::PolicyIndex;

    fn build_ratelimit_policy(name: &str) -> Policy {
        Policy::new(name.to_owned(), Vec::new(), Vec::new(), Vec::new())
    }

    #[test]
    fn not_wildcard_subdomain() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");
        index.insert("example.com", rlp1);

        let val = index.get_longest_match_policy("test.example.com");
        assert!(val.is_none());

        let val = index.get_longest_match_policy("other.com");
        assert!(val.is_none());

        let val = index.get_longest_match_policy("net");
        assert!(val.is_none());

        let val = index.get_longest_match_policy("example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap().name, "rlp1");
    }

    #[test]
    fn wildcard_subdomain_does_not_match_domain() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");

        index.insert("*.example.com", rlp1);
        let val = index.get_longest_match_policy("example.com");
        assert!(val.is_none());
    }

    #[test]
    fn wildcard_subdomain_matches_subdomains() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");

        index.insert("*.example.com", rlp1);
        let val = index.get_longest_match_policy("test.example.com");

        assert!(val.is_some());
        assert_eq!(val.unwrap().name, "rlp1");
    }

    #[test]
    fn longest_domain_match() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");
        index.insert("*.com", rlp1);
        let rlp2 = build_ratelimit_policy("rlp2");
        index.insert("*.example.com", rlp2);

        let val = index.get_longest_match_policy("test.example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap().name, "rlp2");

        let val = index.get_longest_match_policy("example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap().name, "rlp1");
    }

    #[test]
    fn global_wildcard_match_all() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");
        index.insert("*", rlp1);

        let val = index.get_longest_match_policy("test.example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap().name, "rlp1");
    }
}
