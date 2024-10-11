use radix_trie::Trie;
use std::rc::Rc;

use crate::policy::Policy;

pub struct PolicyIndex {
    raw_tree: Trie<String, Vec<Rc<Policy>>>,
}

impl PolicyIndex {
    pub fn new() -> Self {
        Self {
            raw_tree: Trie::new(),
        }
    }

    pub fn insert(&mut self, subdomain: &str, policy: Rc<Policy>) {
        let rev = Self::reverse_subdomain(subdomain);
        self.raw_tree.map_with_default(
            rev,
            |policies| {
                policies.push(Rc::clone(&policy));
            },
            vec![Rc::clone(&policy)],
        );
    }

    pub fn get_longest_match_policies(&self, subdomain: &str) -> Option<&Vec<Rc<Policy>>> {
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
    use std::rc::Rc;

    fn build_ratelimit_policy(name: &str) -> Policy {
        Policy::new(name.to_owned(), Vec::new(), Vec::new())
    }

    #[test]
    fn not_wildcard_subdomain() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");
        index.insert("example.com", Rc::new(rlp1));

        let val = index.get_longest_match_policies("test.example.com");
        assert!(val.is_none());

        let val = index.get_longest_match_policies("other.com");
        assert!(val.is_none());

        let val = index.get_longest_match_policies("net");
        assert!(val.is_none());

        let val = index.get_longest_match_policies("example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap()[0].name, "rlp1");
    }

    #[test]
    fn wildcard_subdomain_does_not_match_domain() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");

        index.insert("*.example.com", Rc::new(rlp1));
        let val = index.get_longest_match_policies("example.com");
        assert!(val.is_none());
    }

    #[test]
    fn wildcard_subdomain_matches_subdomains() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");

        index.insert("*.example.com", Rc::new(rlp1));
        let val = index.get_longest_match_policies("test.example.com");

        assert!(val.is_some());
        assert_eq!(val.unwrap()[0].name, "rlp1");
    }

    #[test]
    fn longest_domain_match() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");
        index.insert("*.com", Rc::new(rlp1));
        let rlp2 = build_ratelimit_policy("rlp2");
        index.insert("*.example.com", Rc::new(rlp2));

        let val = index.get_longest_match_policies("test.example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap()[0].name, "rlp2");

        let val = index.get_longest_match_policies("example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap()[0].name, "rlp1");
    }

    #[test]
    fn global_wildcard_match_all() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy("rlp1");
        index.insert("*", Rc::new(rlp1));

        let val = index.get_longest_match_policies("test.example.com");
        assert!(val.is_some());
        assert_eq!(val.unwrap()[0].name, "rlp1");
    }
}
