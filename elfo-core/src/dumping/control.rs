use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use smallvec::SmallVec;

use elfo_utils::RateLimiter;

use super::config::DumpingConfig;

#[stability::unstable]
#[derive(Default)]
pub struct DumpingControl {
    config: Mutex<DumpingConfig>,
    classes: ArcSwap<SmallVec<[PerClass; 1]>>, // TODO: use `SecondaryMap`?
}

#[derive(Clone)]
struct PerClass {
    class: &'static str,
    disabled: bool,
    limiter: Arc<RateLimiter>, // TODO: use `CachePadded`?
}

impl PerClass {
    fn new(class: &'static str) -> Self {
        Self {
            class,
            disabled: true,
            limiter: Default::default(),
        }
    }

    fn with_config(&self, config: &DumpingConfig) -> Self {
        let limiter = self.limiter.clone();
        limiter.configure(config.max_rate);

        Self {
            class: self.class,
            disabled: config.disabled,
            limiter,
        }
    }

    fn check(&self) -> CheckResult {
        if self.disabled {
            CheckResult::NotInterested
        } else if self.limiter.acquire() {
            CheckResult::Passed
        } else {
            CheckResult::Limited
        }
    }
}

impl DumpingControl {
    pub(crate) fn configure(&self, config: &DumpingConfig) {
        // All structural updates must be performed under the lock.
        let mut config_lock = self.config.lock();
        *config_lock = config.clone();

        let new_classes = self
            .classes
            .load()
            .iter()
            .map(|class| class.with_config(config))
            .collect();

        self.classes.store(Arc::new(new_classes));
    }

    #[stability::unstable]
    pub fn check(&self, class: &'static str) -> CheckResult {
        if let Some(per_class) = find_class(&self.classes.load(), class) {
            per_class.check()
        } else {
            self.add_class(class);
            find_class(&self.classes.load(), class)
                .expect("absent class")
                .check()
        }
    }

    #[cold]
    #[inline(never)]
    fn add_class(&self, class: &'static str) {
        // All structural updates must be performed under the lock.
        let config = self.config.lock();
        let classes = self.classes.load();

        // Check again under the lock.
        if find_class(&classes, class).is_some() {
            return;
        }

        let mut new_classes = (**classes).clone();
        new_classes.push(PerClass::new(class).with_config(&config));
        self.classes.store(Arc::new(new_classes));
    }
}

#[stability::unstable]
pub enum CheckResult {
    Passed,
    NotInterested,
    Limited,
}

fn find_class<'a>(classes: &'a [PerClass], class: &'static str) -> Option<&'a PerClass> {
    classes.iter().find(|c| c.class == class)
}
