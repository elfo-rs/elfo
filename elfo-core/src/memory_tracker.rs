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

        let proc_meminfo = fs::read_to_string(PROC_MEMINFO)
            .map_err(|err| format!("cannot read {PROC_MEMINFO}: {err}"))?;

        let (total, available) = get_values_from_meminfo(&proc_meminfo)?;

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

    fn get_values_from_meminfo(proc_meminfo: &str) -> Result<(usize, usize), String> {
        const MEM_TOTAL_NAME: &str = "MemTotal";
        const MEM_AVAILABLE_NAME: &str = "MemAvailable";

        let mut total = None;
        let mut available = None;

        for line in proc_meminfo.split('\n') {
            let Some((name, suffix)) = line.split_once(':') else {
                continue;
            };

            match name {
                MEM_TOTAL_NAME => {
                    total = Some(
                        parse_size(suffix)
                            .map_err(|err| format!("failed to parse {MEM_TOTAL_NAME}: {err}"))?,
                    )
                }
                MEM_AVAILABLE_NAME => {
                    available =
                        Some(parse_size(suffix).map_err(|err| {
                            format!("failed to parse {MEM_AVAILABLE_NAME}: {err}")
                        })?)
                }
                _ => continue,
            }

            if total.is_some() && available.is_some() {
                break;
            }
        }

        let Some(total) = total else {
            return Err(format!(
                "failed to find {MEM_AVAILABLE_NAME} in {PROC_MEMINFO}"
            ));
        };
        let Some(available) = available else {
            return Err(format!("failed to find {MEM_TOTAL_NAME} in {PROC_MEMINFO}"));
        };

        Ok((total, available))
    }

    fn parse_size(suffix: &str) -> Result<usize, String> {
        suffix
            .trim_start()
            .strip_suffix(" kB")
            .map(|n_str| n_str.parse::<usize>().map_err(|err| err.to_string()))
            .unwrap_or_else(|| {
                Err(format!(
                    "failed to parse amount in {suffix} from {PROC_MEMINFO}"
                ))
            })
            .map(|kb| 1024 * kb)
    }

    #[test]
    fn parsing_works() {
        let linux_correct = "MemTotal:       32446772 kB
MemFree:        27748552 kB
MemAvailable:   27718224 kB
Buffers:           22520 kB
Cached:           256296 kB
SwapCached:        23344 kB";

        let (total, available) = get_values_from_meminfo(linux_correct).unwrap();
        assert_eq!(total, 32446772 * 1024);
        assert_eq!(available, 27718224 * 1024);

        let possible = "MemFree:        27748552 kB
MemTotal:       32446772 kB
Buffers:           22520 kB
Cached:           256296 kB
MemAvailable:   27718224 kB
SwapCached:        23344 kB";

        let (total, available) = get_values_from_meminfo(possible).unwrap();
        assert_eq!(total, 32446772 * 1024);
        assert_eq!(available, 27718224 * 1024);

        let no_total = "MemFree:        27748552 kB
Buffers:           22520 kB
Cached:           256296 kB
MemAvailable:   27718224 kB
SwapCached:        23344 kB";
        get_values_from_meminfo(no_total).unwrap_err();

        let not_kb = "MemTotal:       32446772 kB
MemFree:        27748552 kB
MemAvailable:   27718224 GB
Buffers:           22520 kB
Cached:           256296 kB
SwapCached:        23344 kB";

        get_values_from_meminfo(not_kb).unwrap_err();
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
