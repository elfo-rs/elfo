//! Contains `MemoryTracker` that tracks memory usage of the process.

use metrics::gauge;

/// Checks memory usage to prevent OOM.
pub(crate) struct MemoryTracker {
    threshold: f64,
}

impl MemoryTracker {
    pub(crate) fn new(threshold: f64) -> Result<Self, String> {
        let tracker = Self { threshold };
        tracker.check()?;
        Ok(tracker)
    }

    /// Returns `Ok(false)` if the threshold is reached.
    pub(crate) fn check(&self) -> Result<bool, String> {
        let stats = get_stats()?;
        let used_pct = stats.used as f64 / stats.total as f64;

        gauge!("elfo_memory_usage_bytes", stats.used as f64);

        Ok(used_pct < self.threshold)
    }
}

#[derive(Clone, Copy)]
struct MemoryStats {
    total: usize,
    used: usize,
}

#[cfg(test)]
use mock_stats::get as get_stats;
#[cfg(not(test))]
use proc_stats::get as get_stats;

mod proc_stats {
    use std::fs;

    use super::MemoryStats;

    pub(super) fn get() -> Result<MemoryStats, String> {
        const PROC_SELF_STATM: &str = "/proc/self/statm";
        const PROC_MEMINFO: &str = "/proc/meminfo";
        const PAGE_SIZE: usize = 4096; // TODO: use `sysconf(_SC_PAGESIZE)`

        // Get `used`.
        let proc_meminfo = fs::read_to_string(PROC_MEMINFO)
            .map_err(|err| format!("cannot read {}: {}", PROC_MEMINFO, err))?;

        let total = proc_meminfo
            .split_ascii_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| format!("cannot parse {}", PROC_MEMINFO))?
            * 1024; // always in KiB

        // Get `total`.
        let proc_self_statm = fs::read_to_string(PROC_SELF_STATM)
            .map_err(|err| format!("cannot read {}: {}", PROC_SELF_STATM, err))?;

        let used = proc_self_statm
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| format!("cannot parse {}", PROC_SELF_STATM))?
            * PAGE_SIZE;

        Ok(MemoryStats { used, total })
    }

    #[test]
    fn it_works() {
        let stats = get().unwrap();
        assert!(stats.total > 0);
        assert!(stats.used > 0);
        assert!(stats.used < stats.total);
    }
}

#[cfg(test)]
mod mock_stats {
    use std::cell::Cell;

    use super::MemoryStats;

    thread_local! {
        static STATS: Cell<Result<MemoryStats, &'static str>> = Cell::new(Err("not exists"));
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
    }));

    let memory_tracker = MemoryTracker::new(0.5);
    assert!(memory_tracker.as_ref().unwrap().check().unwrap());

    mock_stats::set(Ok(MemoryStats {
        total: 1000,
        used: 499,
    }));
    assert!(memory_tracker.as_ref().unwrap().check().unwrap());

    mock_stats::set(Ok(MemoryStats {
        total: 1000,
        used: 500,
    }));
    assert!(!memory_tracker.as_ref().unwrap().check().unwrap());
}
