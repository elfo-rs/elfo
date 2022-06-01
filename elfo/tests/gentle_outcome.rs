#![cfg(feature = "test-util")]

use std::panic::AssertUnwindSafe;

use elfo::{
    prelude::*,
    routers::{MapRouter, Outcome},
};
use futures::FutureExt;

#[message]
struct Start(u32);

#[message]
struct GentleUni(u32);

#[message]
struct GentleMulti(Vec<u32>);

#[tokio::test]
async fn it_doesnt_start_actors() {
    let schema = ActorGroup::new()
        .router(MapRouter::new(|e| {
            msg!(match e {
                Start(no) => Outcome::Unicast(*no),
                GentleUni(no) => Outcome::GentleUnicast(*no),
                GentleMulti(nos) => Outcome::GentleMulticast(nos.clone()),
                _ => Outcome::Default,
            })
        }))
        .exec(move |mut ctx| async move {
            msg!(match ctx.recv().await.unwrap() {
                Start(no) => assert_eq!(*ctx.key(), no),
                _ => unreachable!(),
            });

            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    GentleUni(no) => assert_eq!(*ctx.key(), no),
                    GentleMulti(nos) => assert!(nos.contains(ctx.key())),
                    _ => unreachable!(),
                });
            }
        });

    let proxy = elfo::test::proxy(schema, elfo::config::AnyConfig::default()).await;

    // TODO: simplify such tests.
    assert!(AssertUnwindSafe(proxy.send(GentleUni(42)))
        .catch_unwind()
        .await
        .is_err());
    assert!(AssertUnwindSafe(proxy.send(GentleMulti(vec![42, 48])))
        .catch_unwind()
        .await
        .is_err());
    proxy.send(Start(42)).await;
    proxy.send(GentleUni(42)).await;
    proxy.send(GentleMulti(vec![42, 48])).await;
    proxy.send(Start(48)).await;
    proxy.send(GentleUni(48)).await;
}
