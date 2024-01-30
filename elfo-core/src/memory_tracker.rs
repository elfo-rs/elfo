//! Contains `MemoryTracker` that tracks memory usage of the process.
use derive_more::IsVariant;
use metrics::gauge;

/// Checks memory usage to prevent OOM.
pub(crate) struct MemoryTracker {
    /// A threshold that available memory should not reach (in fractions of
    /// total memory).
    available_threshold: f64,
}

#[derive(Debug, Clone, Copy, IsVariant)]
pub(crate) enum MemoryCheckResult {
    Passed,
    Failed(MemoryStats),
}

impl MemoryTracker {
    pub(crate) fn new(threshold: f64) -> Result<Self, String> {
        let tracker = Self {
            available_threshold: 1. - threshold,
        };
        tracker.check()?;
        Ok(tracker)
    }

    /// Returns `Ok(MemoryCheckResult::Failed(stats))` if available memory
    /// amount is below a threshold. `stats` will include memory stats data
    /// that triggered the fail.
    pub(crate) fn check(&self) -> Result<MemoryCheckResult, String> {
        let stats = get_stats()?;

        gauge!("elfo_memory_usage_bytes", stats.used as f64);

        let res = if (stats.available as f64) < stats.total as f64 * self.available_threshold {
            MemoryCheckResult::Failed(stats)
        } else {
            MemoryCheckResult::Passed
        };
        Ok(res)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoryStats {
    pub(crate) total: usize,
    pub(crate) used: usize,
    pub(crate) available: usize,
}

#[cfg(test)]
use mock_stats::get as get_stats;
#[cfg(not(test))]
use proc_stats::get as get_stats;

mod proc_stats {
    use std::fs;

    use super::MemoryStats;

    const PROC_MEMINFO: &str = "/proc/meminfo";

    pub(super) fn get() -> Result<MemoryStats, String> {
        const PROC_SELF_STATM: &str = "/proc/self/statm";
        const PAGE_SIZE: usize = 4096; // TODO: use `sysconf(_SC_PAGESIZE)`

        // Assumes `MemTotal` is always the 1st line in `/proc/meminfo`.
        const MEM_TOTAL_LINE_NO: usize = 0;
        // Assumes `MemAvailable` is always the 3rd line in `/proc/meminfo`.
        const MEM_AVAILABLE_LINE_NO: usize = 2;

        let proc_meminfo = fs::read_to_string(PROC_MEMINFO)
            .map_err(|err| format!("cannot read {PROC_MEMINFO}: {err}"))?;
        let proc_meminfo = proc_meminfo.split_ascii_whitespace();

        let total = get_value_from_meminfo(proc_meminfo.clone(), "MemTotal", MEM_TOTAL_LINE_NO)?;
        let available =
            get_value_from_meminfo(proc_meminfo, "MemAvailable", MEM_AVAILABLE_LINE_NO)?;

        // Get `used`.
        let proc_self_statm = fs::read_to_string(PROC_SELF_STATM)
            .map_err(|err| format!("cannot read {PROC_SELF_STATM}: {err}"))?;

        let used = proc_self_statm
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| format!("cannot parse {PROC_SELF_STATM}"))?
            * PAGE_SIZE;

        Ok(MemoryStats {
            total,
            used,
            available,
        })
    }

    fn get_value_from_meminfo<'a>(
        mut proc_meminfo: impl Iterator<Item = &'a str>,
        item_name: &str,
        line: usize,
    ) -> Result<usize, String> {
        const MEMINFO_ITEM_PER_LINE: usize = 3;
        const MEMINFO_VALUE_INDEX: usize = 1;
        let Some(value) = proc_meminfo.nth(line * MEMINFO_ITEM_PER_LINE + MEMINFO_VALUE_INDEX)
        else {
            return Err(format!(
                "failed to find `{item_name}` at line {line}, in {PROC_MEMINFO}"
            ));
        };

        let value = match value.parse::<usize>() {
            Ok(x) => x * 1024, // always in KiB.
            Err(err) => {
                return Err(format!(
                    "failed to parse `{item_name}`: {err}, in {PROC_MEMINFO}"
                ));
            }
        };
        Ok(value)
    }

    #[test]
    fn it_works() {
        let stats = get().unwrap();
        assert!(stats.total > 0);
        assert!(stats.available > 0);
        assert!(stats.used > 0);
        assert!(stats.used < stats.total);
        assert!(stats.available + stats.used <= stats.total);
    }
}

#[cfg(test)]
mod mock_stats {
    use std::cell::Cell;

    use super::MemoryStats;

    thread_local! {
        static STATS: Cell<Result<MemoryStats, &'static str>> = const { Cell::new(Err("not exists")) };
    }

    pub(super) fn get() -> Result<MemoryStats, String> {
        STATS.with(Cell::get).map_err(Into::into)
    }

    pub(super) fn set(stats: Result<MemoryStats, &'static str>) {
        STATS.with(|cell| cell.set(stats));
    }
}

#[test]
fn it_works() {
    assert!(MemoryTracker::new(0.5).is_err());
    mock_stats::set(Ok(MemoryStats {
        total: 1000,
        used: 100,
        available: 900,
    }));

    let memory_tracker = MemoryTracker::new(0.5);
    assert!(memory_tracker
        .as_ref()
        .unwrap()
        .check()
        .unwrap()
        .is_passed());

    mock_stats::set(Ok(MemoryStats {
        total: 1000,
        used: 200,
        available: 500,
    }));
    assert!(memory_tracker
        .as_ref()
        .unwrap()
        .check()
        .unwrap()
        .is_passed());

    let expected_stats = MemoryStats {
        total: 1000,
        used: 100,
        available: 499,
    };
    mock_stats::set(Ok(expected_stats));
    let res = memory_tracker.as_ref().unwrap().check().unwrap();
    let MemoryCheckResult::Failed(stats) = res else {
        panic!("memory check passed when should have failed");
    };
    assert_eq!(stats, expected_stats);
}
