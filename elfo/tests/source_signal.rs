#![cfg_attr(windows, allow(unused_imports))] // TODO: test on windows
#![allow(clippy::await_holding_lock)]

use elfo::{
    config::AnyConfig,
    prelude::*,
    scope,
    signal::{Signal, SignalKind},
};

// Note: we use different signals for each test to avoid interference.

#[cfg(unix)]
#[tokio::test]
async fn unix() {
    #[message(ret = ())]
    struct Reassign(u64);

    #[message]
    #[derive(PartialEq, Eq)]
    struct SomeSignal(u64);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let signal = ctx.attach(Signal::new(SignalKind::UnixUser1, SomeSignal(0)));
        let mut trace_ids = vec![scope::trace_id()];

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                (Reassign(no), token) => {
                    signal.set_message(SomeSignal(no));
                    trace_ids.push(scope::trace_id());
                    ctx.respond(token, ());
                }
                msg @ SomeSignal => {
                    assert!(!trace_ids.contains(&scope::trace_id()));
                    trace_ids.push(scope::trace_id());
                    ctx.send(msg).await.unwrap();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;

    for no in 0..5 {
        send_signal(libc::SIGUSR1);
        assert_msg_eq!(proxy.recv().await, SomeSignal(no));
        proxy.request(Reassign(no + 1)).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn terminate() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct SomeSignal;

    let group = ActorGroup::new().exec(|mut ctx| async move {
        // Don't use `SIGUSR2` to avoid interference with the configurer.
        let mut signal = Some(ctx.attach(Signal::new(SignalKind::UnixWindowChange, SomeSignal)));

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                SomeSignal => {
                    ctx.send(SomeSignal).await.unwrap();
                    signal.take().unwrap().terminate();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;

    send_signal(libc::SIGWINCH);
    assert_msg_eq!(proxy.recv().await, SomeSignal);

    send_signal(libc::SIGWINCH);
    proxy.sync().await;
    assert!(proxy.try_recv().is_none());
}

#[cfg(unix)]
fn send_signal(signum: libc::c_int) {
    unsafe {
        assert_eq!(libc::kill(libc::getpid(), signum), 0);
    }
}
