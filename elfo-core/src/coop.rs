//! Provides a cooperative budget for actors.
//!
//! # The Problem
//!
//! A single call to an actor's `poll` may potentially do a lot of work before
//! it returns `Poll::Pending`. If an actor runs for a long period of time
//! without yielding back to the executor, it can starve other actors waiting on
//! that executor to execute them, or drive underlying resources. Since Rust
//! does not have a runtime, it is difficult to forcibly preempt a long-running
//! task. Instead, this module provides an opt-in mechanism for actors to
//! collaborate with the executor to avoid starvation.
//!
//! This approach is similar to the one used in tokio.
//!
//! # Supported budgets
//!
//! Elfo supports two kinds of budgeting for actors:
//! * by time: the actor yields to the executor after a certain amount of time,
//!   5s by default.
//! * by count: the actor yields to the executor after a certain number of
//!   [`consume_budget()`] calls, 64 by default.
//!
//! A used kind is determined by the presence of telemetry. It's a controversial
//! decision, but the reason is that telemetry requires fast time source
//! (usually, TSC), which is provided by the `quanta` crate. In this case, time
//! measurements are negligible and a time-based budget is preferred as a more
//! reliable way to prevent starvation.
//!
//! Note that methods [`Context::recv()`] and [`Context::try_recv()`] call
//! [`consume_budget()`] already, so you don't need to think about budgeting
//! in most cases.
//!
//! These limits cannot be configured for now.
//!
//! # Coordination with tokio's budget system
//!
//! Tokio has its own budget system, which is unstable and cannot be used by
//! elfo. Thus, elfo's budget system is independent of tokio's and work
//! simultaneously.
//!
//! However, since both budgets are reset at start of each actor's poll, no
//! matter which budget is exhausted first, both system work coordinately.
//!
//! [`Context::recv()`]: crate::context::Context::recv
//! [`Context::try_recv()`]: crate::context::Context::try_recv

use std::cell::Cell;

use elfo_utils::time::Instant;

// TODO: make it configurable as `system.budget = "5ms" | 64 | "Unlimited"`
const MAX_TIME_NS: u64 = 5_000_000; // 5ms
const MAX_COUNT: u32 = 64;

thread_local! {
    static BUDGET: Cell<Budget> = Cell::new(Budget::ByCount(0));
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq))]
enum Budget {
    /// Used when telemetry is enabled.
    /// We already measure time using `quanta` which is fast.
    ByTime(/* busy_since */ Instant),
    /// Otherwise, limit the number of `recv()` calls.
    ByCount(u32),
}

#[inline]
pub(crate) fn reset(busy_since: Option<Instant>) {
    BUDGET.with(|budget| budget.set(busy_since.map_or(Budget::ByCount(MAX_COUNT), Budget::ByTime)));
}

/// Consumes a unit of budget and returns the execution back to the executor,
/// but only if the actor's coop budget has been exhausted.
///
/// This function can be used in order to insert optional yield points into long
/// computations that do not use `Context::recv()`, `Context::try_recv()` or
/// tokio resources for a long time without redundantly yielding to the executor
/// each time.
///
/// # Example
///
/// Make sure that a function which returns a sum of (potentially lots of)
/// iterated values is cooperative.
///
/// ```
/// # use elfo_core as elfo;
/// async fn sum_iterator(input: &mut impl std::iter::Iterator<Item = i64>) -> i64 {
///     let mut sum: i64 = 0;
///     while let Some(i) = input.next() {
///         sum += i;
///         elfo::coop::consume_budget().await;
///     }
///     sum
/// }
/// ```
#[inline]
pub async fn consume_budget() {
    let to_preempt = BUDGET.with(|cell| {
        let budget = cell.get();

        match budget {
            Budget::ByTime(busy_since) => Instant::now().nanos_since(busy_since) >= MAX_TIME_NS,
            Budget::ByCount(0) => true,
            Budget::ByCount(left) => {
                cell.set(Budget::ByCount(left - 1));
                false
            }
        }
    });

    if to_preempt {
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::{pin, Pin},
        task::{Context, Poll},
        time::Duration,
    };

    use pin_project::pin_project;
    use tokio::runtime::Builder;

    use elfo_utils::time::with_instant_mock;

    use super::*;

    fn current_budget() -> Budget {
        BUDGET.with(Cell::get)
    }

    #[pin_project]
    struct ResetOnPoll<F>(/* use time */ bool, #[pin] F);

    impl<F: Future> Future for ResetOnPoll<F> {
        type Output = F::Output;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.project();
            reset(if *this.0 { Some(Instant::now()) } else { None });
            this.1.poll(cx)
        }
    }

    #[test]
    fn by_count() {
        let rt = Builder::new_current_thread().build().unwrap();

        let task = async {
            assert_eq!(current_budget(), current_budget());

            for _ in 0..10 {
                for i in (0..=MAX_COUNT).rev() {
                    assert_eq!(current_budget(), Budget::ByCount(i));
                    consume_budget().await;
                }
            }
        };

        rt.block_on(ResetOnPoll(false, task));
    }

    #[test]
    fn by_time() {
        let rt = Builder::new_current_thread().build().unwrap();
        let steps = 10;
        let timestep = Duration::from_nanos(MAX_TIME_NS / steps);

        with_instant_mock(|mock| {
            let task = async move {
                assert_eq!(current_budget(), current_budget());

                for _ in 0..10 {
                    let before = current_budget();
                    assert!(matches!(before, Budget::ByTime(_)));

                    for _ in 0..steps {
                        mock.advance(timestep);
                        assert_eq!(current_budget(), before);
                        consume_budget().await;
                    }

                    assert_ne!(current_budget(), before);
                }
            };

            rt.block_on(ResetOnPoll(true, task));
        })
    }
}
