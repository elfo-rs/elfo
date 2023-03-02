use std::collections::hash_map::Entry;

use fxhash::FxHashMap;
use tracing::level_filters::LevelFilter;

use elfo_core::dumping::MessageName;

use crate::config::{OnOverflow, Rule};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DumpParams {
    pub(crate) max_size: usize,
    pub(crate) on_overflow: OnOverflow,
    pub(crate) on_overflow_log: LevelFilter,
    pub(crate) on_failure_log: LevelFilter,
}

impl Default for DumpParams {
    fn default() -> Self {
        Self {
            max_size: 64 * 1024,
            on_overflow: OnOverflow::Skip,
            on_overflow_log: LevelFilter::WARN,
            on_failure_log: LevelFilter::WARN,
        }
    }
}

pub(crate) struct RuleSet {
    class: &'static str,
    rules: Vec<Rule>,
    cache: FxHashMap<(&'static str, MessageName), DumpParams>,
}

impl RuleSet {
    pub(crate) fn new(class: &'static str) -> Self {
        Self {
            class,
            rules: vec![],
            cache: FxHashMap::default(),
        }
    }

    pub(crate) fn configure(&mut self, rules: &[Rule]) {
        let iter = rules
            .iter()
            .filter(|rule| rule.class.as_ref().map_or(true, |c| c == self.class));

        if self.rules.iter().ne(iter.clone()) {
            self.cache.clear();
            self.rules = iter.cloned().collect();
        }
    }

    pub(crate) fn get(&mut self, protocol: &'static str, message: &MessageName) -> &DumpParams {
        self.do_get(protocol, message).1
    }

    fn do_get(&mut self, protocol: &'static str, message: &MessageName) -> (bool, &DumpParams) {
        match self.cache.entry((protocol, message.clone())) {
            Entry::Occupied(entry) => (true, entry.into_mut()),
            Entry::Vacant(entry) => (
                false,
                entry.insert(collect_params(&self.rules, protocol, message)),
            ),
        }
    }
}

#[cold]
fn collect_params(rules: &[Rule], protocol: &'static str, message: &MessageName) -> DumpParams {
    let mut params = DumpParams::default();

    rules
        .iter()
        .filter(|r| {
            r.protocol.as_ref().map_or(true, |p| p == protocol)
                && r.message.as_ref().map_or(true, |m| &m.as_str() == message)
        })
        .for_each(|r| {
            params.max_size = r.max_size.unwrap_or(params.max_size);
            params.on_overflow = r.on_overflow.unwrap_or(params.on_overflow);
            params.on_overflow_log = r.on_overflow_log.unwrap_or(params.on_overflow_log);
            params.on_failure_log = r.on_failure_log.unwrap_or(params.on_failure_log);
        });

    params
}

#[test]
fn it_works() {
    let mut rules = vec![
        Rule {
            class: Some("another".into()),
            max_size: Some(0),
            ..Rule::default()
        },
        Rule {
            class: Some("some".into()),
            protocol: Some("proto_a".into()),
            max_size: Some(1),
            ..Rule::default()
        },
        Rule {
            message: Some("A".into()),
            max_size: Some(2),
            ..Rule::default()
        },
        Rule {
            protocol: Some("proto_b".into()),
            message: Some("B".into()),
            max_size: Some(3),
            on_overflow_log: Some(LevelFilter::INFO),
            ..Rule::default()
        },
        Rule {
            message: Some("B".into()),
            max_size: Some(4),
            on_failure_log: Some(LevelFilter::ERROR),
            ..Rule::default()
        },
    ];

    let mut set = RuleSet::new("some");
    set.configure(&rules);

    // No rules are applied.
    assert_eq!(
        set.do_get("unused_proto", &"U".into()),
        (false, &DumpParams::default())
    );
    // From the cache.
    assert!(set.do_get("unused_proto", &"U".into()).0);

    // One rule is applied.
    assert_eq!(
        set.do_get("proto_b", &"A".into()),
        (
            false,
            &DumpParams {
                max_size: 2,
                ..DumpParams::default()
            }
        )
    );
    // From the cache.
    assert!(set.do_get("proto_b", &"A".into()).0);

    // Multiple rules are applied.
    assert_eq!(
        set.do_get("proto_a", &"A".into()),
        (
            false,
            &DumpParams {
                max_size: 2,
                ..DumpParams::default()
            }
        )
    );
    // From the cache.
    assert!(set.do_get("proto_a", &"A".into()).0);

    // Multiple rules are applied, params are merged.
    assert_eq!(
        set.do_get("proto_b", &"B".into()),
        (
            false,
            &DumpParams {
                max_size: 4,
                on_overflow_log: LevelFilter::INFO,
                on_failure_log: LevelFilter::ERROR,
                ..DumpParams::default()
            }
        )
    );
    // From the cache.
    assert!(set.do_get("proto_b", &"B".into()).0);

    // Reconfiguration with the same rules for the class doesn't reset the cache.
    rules.remove(0);
    set.configure(&rules);
    assert!(set.do_get("proto_a", &"A".into()).0);
    assert!(set.do_get("proto_b", &"B".into()).0);

    // Reconfiguration with different rules resets the cache.
    rules.remove(0);
    set.configure(&rules);
    assert!(!set.do_get("proto_a", &"A".into()).0);
    assert!(!set.do_get("proto_b", &"B".into()).0);
}
