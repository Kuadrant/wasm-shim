use crate::runtime_action_set::RuntimeActionSet;
use radix_trie::Trie;
use std::rc::Rc;

pub(crate) struct ActionSetIndex {
    raw_tree: Trie<String, Vec<Rc<RuntimeActionSet>>>,
}

impl ActionSetIndex {
    pub fn new() -> Self {
        Self {
            raw_tree: Trie::new(),
        }
    }

    pub fn insert(&mut self, subdomain: &str, action_set: Rc<RuntimeActionSet>) {
        let rev = Self::reverse_subdomain(subdomain);
        self.raw_tree.map_with_default(
            rev,
            |action_sets| {
                action_sets.push(Rc::clone(&action_set));
            },
            vec![Rc::clone(&action_set)],
        );
    }

    pub fn get_longest_match_action_sets(
        &self,
        subdomain: &str,
    ) -> Option<&Vec<Rc<RuntimeActionSet>>> {
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
    use crate::action_set_index::ActionSetIndex;
    use crate::runtime_action_set::RuntimeActionSet;
    use std::rc::Rc;

    fn build_ratelimit_action_set(name: &str) -> RuntimeActionSet {
        RuntimeActionSet {
            name: name.to_owned(),
            route_rule_predicates: Default::default(),
            runtime_actions: Vec::new(),
        }
    }

    #[test]
    fn not_wildcard_subdomain() {
        let mut index = ActionSetIndex::new();
        let rlp1 = build_ratelimit_action_set("rlp1");
        index.insert("example.com", Rc::new(rlp1));

        let val = index.get_longest_match_action_sets("test.example.com");
        assert!(val.is_none());

        let val = index.get_longest_match_action_sets("other.com");
        assert!(val.is_none());

        let val = index.get_longest_match_action_sets("net");
        assert!(val.is_none());

        let val = index.get_longest_match_action_sets("example.com");
        assert!(val.is_some());
        assert_eq!(val.expect("value must be some")[0].name, "rlp1");
    }

    #[test]
    fn wildcard_subdomain_does_not_match_domain() {
        let mut index = ActionSetIndex::new();
        let rlp1 = build_ratelimit_action_set("rlp1");

        index.insert("*.example.com", Rc::new(rlp1));
        let val = index.get_longest_match_action_sets("example.com");
        assert!(val.is_none());
    }

    #[test]
    fn wildcard_subdomain_matches_subdomains() {
        let mut index = ActionSetIndex::new();
        let rlp1 = build_ratelimit_action_set("rlp1");

        index.insert("*.example.com", Rc::new(rlp1));
        let val = index.get_longest_match_action_sets("test.example.com");

        assert!(val.is_some());
        assert_eq!(val.expect("value must be some")[0].name, "rlp1");
    }

    #[test]
    fn longest_domain_match() {
        let mut index = ActionSetIndex::new();
        let rlp1 = build_ratelimit_action_set("rlp1");
        index.insert("*.com", Rc::new(rlp1));
        let rlp2 = build_ratelimit_action_set("rlp2");
        index.insert("*.example.com", Rc::new(rlp2));

        let val = index.get_longest_match_action_sets("test.example.com");
        assert!(val.is_some());
        assert_eq!(val.expect("value must be some")[0].name, "rlp2");

        let val = index.get_longest_match_action_sets("example.com");
        assert!(val.is_some());
        assert_eq!(val.expect("value must be some")[0].name, "rlp1");
    }

    #[test]
    fn global_wildcard_match_all() {
        let mut index = ActionSetIndex::new();
        let rlp1 = build_ratelimit_action_set("rlp1");
        index.insert("*", Rc::new(rlp1));

        let val = index.get_longest_match_action_sets("test.example.com");
        assert!(val.is_some());
        assert_eq!(val.expect("value must be some")[0].name, "rlp1");
    }
}
