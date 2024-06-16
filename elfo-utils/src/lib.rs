//! A collection of utilities to share among elfo-* crates.

pub use self::{
    likely::*,
    rate_limiter::{RateLimit, RateLimiter},
};

mod likely;
mod rate_limiter;
pub mod time;

pub use crossbeam_utils::CachePadded;

/// Returns the contents of a `Option<T>`'s `Some(T)`, otherwise it returns
/// early from the function. Can alternatively have an `else` branch, or an
/// alternative "early return" statement, like `break` or `continue` for loops.
#[macro_export]
macro_rules! ward {
    ($o:expr) => {
        // Do not reuse `ward!` here, because it confuses rust-analyzer for now.
        match $o {
            Some(x) => x,
            None => return,
        }
    };
    ($o:expr, else $body:block) => {
        match $o {
            Some(x) => x,
            None => $body,
        }
    };
    ($o:expr, $early:stmt) => {
        // Do not reuse `ward!` here, because it confuses rust-analyzer for now.
        match $o {
            Some(x) => x,
            None => ({ $early }),
        }
    };
}

/// A simple macro to check if a cooldown period has passed since the last time.
#[macro_export]
macro_rules! cooldown {
    ($period:expr) => {{
        use ::std::{
            sync::atomic::{AtomicU64, Ordering},
            time::UNIX_EPOCH,
        };

        static LOGGED_TIME: AtomicU64 = AtomicU64::new(0);

        let period = $period.as_nanos() as u64;
        let res = LOGGED_TIME.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |logged_time| {
            let now = UNIX_EPOCH.elapsed().unwrap_or_default().as_nanos() as u64;
            if logged_time + period <= now {
                Some(now)
            } else {
                None
            }
        });

        res.is_ok()
    }};
    ($period:expr, $body:expr) => {{
        #[deprecated(note = "use `if cooldown!(duration) { .. }` instead")]
        fn deprecation() {}

        if $crate::cooldown!($period) {
            deprecation();
            $body
        }
    }};
}
