use std::{
    collections::hash_map::Entry, error::Error as StdError, hash::Hash, mem, time::Duration,
};

use fxhash::FxHashMap;
use metrics::counter;
use tokio::time::Instant;
use tracing::{debug, error, info, trace, warn, Level};

use elfo_core::dumping::{Dump, MessageName};
use elfo_utils::ward;

use crate::rule_set::DumpParams;

type MessageProtocol = &'static str;

// === Report ===

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) appended: usize,
    pub(crate) failed: FxHashMap<(MessageProtocol, MessageName), FailedDumpInfo>,
    pub(crate) overflow: FxHashMap<(MessageProtocol, MessageName, bool), OverflowDumpInfo>,
    // If new fields are added, update `Report::merge()`.
}

#[derive(Debug)]
pub(crate) struct FailedDumpInfo {
    pub(crate) level: Level,
    pub(crate) error: serde_json::Error,
    pub(crate) count: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OverflowDumpInfo {
    pub(crate) level: Level,
    pub(crate) count: usize,
}

impl Report {
    #[cold]
    pub(crate) fn add_failed(
        &mut self,
        dump: &Dump,
        error: serde_json::Error,
        params: &DumpParams,
    ) {
        let level = ward!(params.log_on_failure.into_level());

        self.failed
            .entry((dump.message_protocol, dump.message_name.clone()))
            .and_modify(|info| {
                info.level = level;
                info.count += 1;
            })
            .or_insert_with(|| FailedDumpInfo {
                level,
                error,
                count: 1,
            });
    }

    #[cold]
    pub(crate) fn add_overflow(&mut self, dump: &Dump, truncated: bool, params: &DumpParams) {
        let level = ward!(params.log_on_overflow.into_level());

        self.overflow
            .entry((dump.message_protocol, dump.message_name.clone(), truncated))
            .and_modify(|info| {
                info.level = level;
                info.count += 1;
            })
            .or_insert_with(|| OverflowDumpInfo { level, count: 1 });
    }

    pub(crate) fn merge(&mut self, another: Report) {
        self.appended += another.appended;

        merge_maps(&mut self.failed, another.failed, |this, that| {
            this.level = that.level;
            this.count += that.count;
        });
        merge_maps(&mut self.overflow, another.overflow, |this, that| {
            this.level = that.level;
            this.count += that.count;
        });
    }
}

fn merge_maps<K: Eq + Hash, V>(
    dest: &mut FxHashMap<K, V>,
    src: FxHashMap<K, V>,
    f: impl Fn(&mut V, V),
) {
    for (key, that) in src {
        match dest.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(that);
            }
            Entry::Occupied(mut entry) => f(entry.get_mut(), that),
        }
    }
}

// === Reporter ===

pub(crate) struct Reporter {
    report: Report,
    last_report_time: Option<Instant>,
    log_cooldown: Duration,
}

macro_rules! event_dyn_level {
    ($level:expr, $($t:tt)*) => {
        if $level == Level::ERROR {
            error!($($t)*)
        } else if $level == Level::WARN {
            warn!($($t)*)
        } else if $level == Level::INFO {
            info!($($t)*)
        } else if $level == Level::DEBUG {
            debug!($($t)*)
        } else if $level == Level::TRACE {
            trace!($($t)*)
        }
    }
}

impl Reporter {
    pub(crate) fn new(log_cooldown: Duration) -> Self {
        Self {
            report: Report::default(),
            last_report_time: None,
            log_cooldown,
        }
    }

    pub(crate) fn configure(&mut self, log_cooldown: Duration) {
        self.log_cooldown = log_cooldown;
    }

    pub(crate) fn add(&mut self, report: Report) {
        self.report.merge(report);
        self.emit(false);
    }

    fn emit(&mut self, force: bool) {
        // Emit metrics immediately, they are combined by the telemetry system.
        counter!("elfo_written_dumps_total", self.report.appended as u64);
        self.report.appended = 0;

        // Throttle logs to produce less noise.
        if !(force || self.should_log()) {
            return;
        }

        let report = mem::take(&mut self.report);

        for ((protocol, name), info) in report.failed {
            event_dyn_level!(
                info.level,
                message = "cannot serialize message, skipped",
                protocol = %protocol,
                name = %name,
                error = &info.error as &(dyn StdError),
                count = info.count,
            );
        }

        for ((protocol, name, truncated), info) in report.overflow {
            event_dyn_level!(
                info.level,
                message = if truncated {
                    "too big dump, truncated"
                } else {
                    "too big dump, skipped"
                },
                protocol = %protocol,
                name = %name,
                count = info.count,
            );
        }

        self.last_report_time = Some(Instant::now());
    }

    fn should_log(&self) -> bool {
        if self.report.failed.is_empty() && self.report.overflow.is_empty() {
            return false;
        }

        self.last_report_time
            .map_or(true, |t| t.elapsed() >= self.log_cooldown)
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        self.emit(true);
    }
}
