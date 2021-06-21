#![cfg(feature = "full")]

use std::convert::TryFrom;

use elfo::{config::AnyConfig, prelude::*, stream, tls, trace_id::TraceId};

#[message]
#[derive(PartialEq)]
struct SomeMessage(u32);

#[message]
struct Set(Vec<u32>);

#[message]
struct Replace(Vec<u32>);

#[tokio::test]
async fn it_handles_basic_operations() {
    let group = ActorGroup::new().exec(|ctx| async move {
        let stream = stream::Stream::new(futures::stream::iter(vec![SomeMessage(0)]));

        let mut ctx = ctx.with(&stream);
        let mut prev_trace_id = tls::trace_id();

        while let Some(envelope) = ctx.recv().await {
            assert_ne!(tls::trace_id(), prev_trace_id);
            prev_trace_id = tls::trace_id();

            msg!(match envelope {
                m @ SomeMessage(_) => ctx.send(m).await.unwrap(),
                Set(d) => stream.set(futures::stream::iter(
                    d.into_iter().map(SomeMessage).collect::<Vec<_>>()
                )),
                Replace(d) => {
                    let _ = stream.replace(futures::stream::iter(
                        d.into_iter().map(SomeMessage).collect::<Vec<_>>(),
                    ));
                }
            })
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert_msg_eq!(proxy.recv().await, SomeMessage(0));
    assert!(proxy.try_recv().is_none());

    proxy.send(Set((1..5).collect())).await;
    for i in 1..5 {
        assert_msg_eq!(proxy.recv().await, SomeMessage(i));
    }
    assert!(proxy.try_recv().is_none());

    proxy.send(Replace((6..8).collect())).await;
    for i in 6..8 {
        assert_msg_eq!(proxy.recv().await, SomeMessage(i));
    }
}

#[tokio::test]
async fn it_restores_trace_id() {
    let group = ActorGroup::new().exec(|ctx| async move {
        let stream = stream::Stream::new(futures::stream::iter(vec![
            (TraceId::try_from(5).unwrap(), SomeMessage(5)),
            (TraceId::try_from(6).unwrap(), SomeMessage(6)),
        ]));

        let mut ctx = ctx.with(&stream);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                SomeMessage(x) => {
                    assert_eq!(u64::from(tls::trace_id()), u64::from(x));
                    ctx.send(SomeMessage(x)).await.unwrap()
                }
            })
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert_msg_eq!(proxy.recv().await, SomeMessage(5));
    assert_msg_eq!(proxy.recv().await, SomeMessage(6));
}
