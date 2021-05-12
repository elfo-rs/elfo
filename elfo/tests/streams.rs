#![cfg(feature = "full")]

use elfo::{config::AnyConfig, prelude::*, stream};

#[message]
#[derive(PartialEq)]
struct SomeMessage(u32);

#[message]
struct Set(Vec<u32>);

#[message]
struct Replace(Vec<u32>);

#[message]
#[derive(PartialEq)]
struct EndOfMessages;

#[tokio::test]
async fn it_handles_basic_operations() {
    let group = ActorGroup::new().exec(|ctx| async move {
        let stream = stream::Stream::new(futures::stream::iter(vec![SomeMessage(0)]));

        let mut ctx = ctx.with(&stream);

        while let Some(envelope) = ctx.recv().await {
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
                EndOfMessages => ctx.send(EndOfMessages).await.unwrap(),
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
