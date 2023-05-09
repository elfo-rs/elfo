use std::{collections::HashMap, time::Duration};

use elfo::{config::AnyConfig, prelude::*, scope, time::Delay};

#[tokio::test(start_paused = true)]
async fn smoke() {
    #[message]
    struct Start(u64);

    #[message]
    #[derive(PartialEq, Eq)]
    struct Tick(u64);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let mut handles = HashMap::new();
        let mut terminated = None::<Delay<_>>;

        while let Some(envelope) = ctx.recv().await {
            if let Some(handle) = terminated.take() {
                assert!(handle.is_terminated());
            }

            msg!(match envelope {
                Start(group) => {
                    let delay = Duration::from_millis(group);
                    let handle = ctx.attach(Delay::new(delay, Tick(group)));
                    handles.insert(group, (handle, scope::trace_id()));
                }
                msg @ Tick(group) => {
                    let (handle, expected_trace_id) = handles.remove(&group).unwrap();
                    assert_eq!(scope::trace_id(), expected_trace_id);
                    ctx.send(msg).await.unwrap();

                    assert!(!handle.is_terminated()); // become after the next `recv()`
                    assert!(terminated.is_none());
                    terminated = Some(handle);
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert!(proxy.try_recv().is_none());

    proxy.send(Start(100)).await;
    proxy.send(Start(25)).await;
    proxy.send(Start(50)).await;

    assert_msg_eq!(proxy.recv().await, Tick(25));
    proxy.send(Start(5)).await;

    assert_msg_eq!(proxy.recv().await, Tick(5));
    assert_msg_eq!(proxy.recv().await, Tick(50));
    assert_msg_eq!(proxy.recv().await, Tick(100));
    assert!(proxy.try_recv().is_none());

    proxy.send(Start(10)).await;
    proxy.send(Start(15)).await;
    assert_msg_eq!(proxy.recv().await, Tick(10));
    assert_msg_eq!(proxy.recv().await, Tick(15));
}

#[tokio::test(start_paused = true)]
async fn terminate() {
    #[message]
    struct Start(u64);

    #[message]
    struct Terminate(u64);

    #[message]
    #[derive(PartialEq, Eq)]
    struct Tick(u64);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let mut handles = HashMap::new();

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Start(group) => {
                    let delay = Duration::from_millis(group);
                    let handle = ctx.attach(Delay::new(delay, Tick(group)));
                    handles.insert(group, handle);
                }
                Terminate(group) => {
                    let handle = handles.remove(&group).unwrap();
                    assert!(!handle.is_terminated());
                    handle.terminate();
                }
                Tick(group) => {
                    ctx.send(Tick(group)).await.unwrap();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert!(proxy.try_recv().is_none());

    proxy.send(Start(10)).await;
    proxy.send(Start(20)).await;
    proxy.send(Start(30)).await;

    assert_msg_eq!(proxy.recv().await, Tick(10));
    proxy.send(Terminate(20)).await;
    assert_msg_eq!(proxy.recv().await, Tick(30));
}
