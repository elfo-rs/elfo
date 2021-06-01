#![cfg(feature = "test-util")]

use elfo::prelude::*;

#[message]
struct Terminate;

#[message]
struct Terminated;

#[tokio::test]
async fn it_restarts_explicitly() {
    let schema = ActorGroup::new().exec(move |mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Terminate { .. } => break,
                _ => unreachable!(),
            });
        }
        ctx.send(Terminated).await.unwrap();
    });

    let mut proxy = elfo::test::proxy(schema, elfo::config::AnyConfig::default()).await;
    proxy.send(Terminate).await;
    assert_msg!(proxy.recv().await, Terminated);

    proxy.send(Terminate).await;
    assert_msg!(proxy.recv().await, Terminated);
}
