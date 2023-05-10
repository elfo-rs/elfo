use std::time::Duration;

use elfo::{config::AnyConfig, prelude::*, scope, test::Proxy, time::Interval};
use tokio::time::{sleep, Instant};

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

#[tokio::test(start_paused = true)]
async fn multiple() {
    #[message]
    struct Start(Duration);

    #[message]
    #[derive(PartialEq, Eq)]
    struct Tick(Duration);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let mut trace_ids = Vec::new();

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Start(duration) => {
                    trace_ids.push(scope::trace_id());
                    ctx.attach(Interval::new(Tick(duration))).start(duration);
                }
                msg @ Tick => {
                    assert!(!trace_ids.contains(&scope::trace_id()));
                    ctx.send(msg).await.unwrap();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert!(proxy.try_recv().await.is_none());

    proxy.send(Start(ms(11))).await;
    proxy.send(Start(ms(35))).await;
    proxy.send(Start(ms(49))).await;

    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 11
    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 22
    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 33
    assert_msg_eq!(proxy.recv().await, Tick(ms(35))); // 35
    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 44
    assert_msg_eq!(proxy.recv().await, Tick(ms(49))); // 49
    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 55
    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 66
    assert_msg_eq!(proxy.recv().await, Tick(ms(35))); // 70
    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 77
    assert_msg_eq!(proxy.recv().await, Tick(ms(11))); // 88
    assert_msg_eq!(proxy.recv().await, Tick(ms(49))); // 98
}

#[message]
struct Start(Duration);

#[message]
struct StartAfter(Duration, Duration);

#[message]
struct Stop;

#[message]
struct SetPeriod(Duration);

#[message]
struct SetMessage(u32);

#[message]
#[derive(PartialEq, Eq)]
struct Tick(u32);

#[message]
struct Terminate;

fn sample() -> Blueprint {
    ActorGroup::new().exec(move |mut ctx| async move {
        let mut interval = Some(ctx.attach(Interval::new(Tick(0))));

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Start(period) => {
                    interval.as_ref().unwrap().start(period);
                }
                StartAfter(delay, period) => {
                    interval.as_ref().unwrap().start_after(delay, period);
                }
                Stop => {
                    interval.as_ref().unwrap().stop();
                }
                SetPeriod(period) => {
                    interval.as_ref().unwrap().set_period(period)
                }
                SetMessage(no) => {
                    interval.as_ref().unwrap().set_message(Tick(no));
                }
                Terminate => {
                    interval.take().unwrap().terminate();
                }
                msg @ Tick => {
                    ctx.send(msg).await.unwrap();
                }
            });
        }
    })
}

struct Checker {
    proxy: Proxy,
    prev_time: Instant,
}

impl Checker {
    fn new(proxy: Proxy) -> Self {
        Self {
            proxy,
            prev_time: Instant::now(),
        }
    }

    async fn send<M: elfo::Message>(&self, message: M) {
        self.proxy.send(message).await;
    }

    async fn tick(&mut self, expected_elapsed: Duration, expected_no: u32) {
        assert_msg_eq!(self.proxy.recv().await, Tick(expected_no));

        let now = Instant::now();
        let elapsed = now - self.prev_time;
        assert_eq!(elapsed, expected_elapsed);

        self.prev_time = now;
    }
}

#[tokio::test(start_paused = true)]
async fn start_after() {
    let proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    let mut checker = Checker::new(proxy);

    checker.send(StartAfter(ms(15), ms(10))).await;
    checker.tick(ms(15), 0).await;
    checker.tick(ms(10), 0).await;
    checker.tick(ms(10), 0).await;
}

#[tokio::test(start_paused = true)]
async fn restart() {
    let proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    let mut checker = Checker::new(proxy);

    checker.send(Start(ms(10))).await;
    checker.tick(ms(10), 0).await;

    sleep(ms(5)).await;
    checker.send(Start(ms(13))).await;
    checker.tick(ms(18), 0).await;
    checker.tick(ms(13), 0).await;
}

#[tokio::test(start_paused = true)]
async fn stop_start() {
    let proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    let mut checker = Checker::new(proxy);

    checker.send(Start(ms(10))).await;
    checker.tick(ms(10), 0).await;

    sleep(ms(5)).await;
    checker.send(Stop).await;

    sleep(ms(40)).await;
    checker.send(Start(ms(13))).await;
    checker.tick(ms(58), 0).await;
    checker.tick(ms(13), 0).await;
}

#[tokio::test(start_paused = true)]
async fn set_message() {
    let proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    let mut checker = Checker::new(proxy);

    checker.send(Start(ms(10))).await;
    checker.tick(ms(10), 0).await;
    checker.send(SetMessage(1)).await;
    checker.tick(ms(10), 1).await;
    checker.send(SetMessage(2)).await;
    checker.tick(ms(10), 2).await;
}

#[tokio::test(start_paused = true)]
async fn set_period() {
    let proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    let mut checker = Checker::new(proxy);

    checker.send(Start(ms(10))).await;
    checker.tick(ms(10), 0).await;

    sleep(ms(7)).await;
    checker.send(SetPeriod(ms(15))).await;
    checker.tick(ms(15), 0).await;

    sleep(ms(10)).await;
    checker.send(SetPeriod(ms(5))).await;
    checker.tick(ms(10), 0).await;
}

#[tokio::test(start_paused = true)]
async fn burst() {
    let proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    let mut checker = Checker::new(proxy);

    checker.send(Start(ms(10))).await;
    sleep(ms(35)).await;

    checker.tick(ms(35), 0).await;
    checker.tick(ms(0), 0).await;
    checker.tick(ms(0), 0).await;
    checker.tick(ms(5), 0).await;
    checker.tick(ms(10), 0).await;
}

#[tokio::test(start_paused = true)]
async fn terminate() {
    let mut proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;

    proxy.send(Start(ms(10))).await;
    assert_msg_eq!(proxy.recv().await, Tick(0));

    proxy.send(Terminate).await;
    proxy.sync().await;
    assert!(proxy.try_recv().await.is_none());
}
