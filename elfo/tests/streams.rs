#![cfg(feature = "full")]

use elfo::{config::AnyConfig, prelude::*, stream};

#[message]
#[derive(PartialEq)]
struct SomeMessage(u32);

#[message]
#[derive(PartialEq)]
struct EndOfMessages;

fn samples() -> Schema {
    ActorGroup::new().exec(|ctx| async move {
        let stream1 =
            stream::Stream::new(futures::stream::iter(vec![SomeMessage(0), SomeMessage(1)]));

        let mut ctx = ctx.with(stream1);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                m @ SomeMessage(_) => ctx.send(m).await.unwrap(),
                EndOfMessages => ctx.send(EndOfMessages).await.unwrap(),
            })
        }
    })
}

#[tokio::test]
async fn it_works() {
    let mut proxy = elfo::test::proxy(samples(), AnyConfig::default()).await;

    assert_msg_eq!(proxy.recv().await, SomeMessage(0));
    assert_msg_eq!(proxy.recv().await, SomeMessage(1));
}
