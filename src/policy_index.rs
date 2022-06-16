use radix_trie::Trie;

use crate::configuration::RateLimitPolicy;

pub struct PolicyIndex {
    raw_tree: Trie<String, RateLimitPolicy>,
}

impl PolicyIndex {
    pub fn new() -> Self {
        Self {
            raw_tree: Trie::new(),
        }
    }

    pub fn insert(&mut self, subdomain: &str, policy: RateLimitPolicy) {
        let rev = Self::reverse_subdomain(subdomain);
        self.raw_tree.insert(rev, policy);
    }

    pub fn get_longest_match_policy(&self, subdomain: &str) -> Option<&RateLimitPolicy> {
        let rev = Self::reverse_subdomain(subdomain);
        self.raw_tree.get_ancestor_value(&rev)
    }

    fn reverse_subdomain(subdomain: &str) -> String {
        let mut s = subdomain.to_string();
        s.push('.');
        if s.chars().nth(0).unwrap() == '*' {
            s.remove(0);
        } else {
            s.insert(0, '$'); // $ is not a valid domain char
        }
        s.chars().rev().collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::configuration::RateLimitPolicy;
    use crate::glob::GlobPatternSet;
    use crate::policy_index::PolicyIndex;

    fn build_ratelimit_policy(domain: String) -> RateLimitPolicy {
        RateLimitPolicy::new(GlobPatternSet::default(), None, None, None, Some(domain))
    }

    #[test]
    fn not_wildcard_subdomain() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy(String::from("example.com"));
        index.insert("example.com", rlp1);

        let val = index.get_longest_match_policy("test.example.com");
        assert_eq!(val.is_none(), true);

        let val = index.get_longest_match_policy("other.com");
        assert_eq!(val.is_none(), true);

        let val = index.get_longest_match_policy("net");
        assert_eq!(val.is_none(), true);

        let val = index.get_longest_match_policy("example.com");
        assert_eq!(val.is_some(), true);
        assert_eq!(val.unwrap().domain().is_some(), true);
        assert_eq!(val.unwrap().domain().unwrap(), "example.com");
    }

    #[test]
    fn wildcard_subdomain_does_not_match_domain() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy(String::from("*.example.com"));

        index.insert("*.example.com", rlp1);
        let val = index.get_longest_match_policy("example.com");
        assert_eq!(val.is_none(), true);
    }

    #[test]
    fn wildcard_subdomain_matches_subdomains() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy(String::from("*.example.com"));

        index.insert("*.example.com", rlp1);
        let val = index.get_longest_match_policy("test.example.com");

        assert_eq!(val.is_some(), true);
        assert_eq!(val.unwrap().domain().is_some(), true);
        assert_eq!(val.unwrap().domain().unwrap(), "*.example.com");
    }

    #[test]
    fn longest_domain_match() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy(String::from("*.com"));
        index.insert("*.com", rlp1);
        let rlp2 = build_ratelimit_policy(String::from("*.example.com"));
        index.insert("*.example.com", rlp2);

        let val = index.get_longest_match_policy("test.example.com");
        assert_eq!(val.is_some(), true);
        assert_eq!(val.unwrap().domain().is_some(), true);
        assert_eq!(val.unwrap().domain().unwrap(), "*.example.com");

        let val = index.get_longest_match_policy("example.com");
        assert_eq!(val.is_some(), true);
        assert_eq!(val.unwrap().domain().is_some(), true);
        assert_eq!(val.unwrap().domain().unwrap(), "*.com");
    }

    #[test]
    fn global_wildcard_match_all() {
        let mut index = PolicyIndex::new();
        let rlp1 = build_ratelimit_policy(String::from("*"));
        index.insert("*", rlp1);

        let val = index.get_longest_match_policy("test.example.com");
        assert_eq!(val.is_some(), true);
        assert_eq!(val.unwrap().domain().is_some(), true);
        assert_eq!(val.unwrap().domain().unwrap(), "*");
    }
}
