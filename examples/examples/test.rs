use elfo::prelude::*;
use serde::Deserialize;

#[message]
struct Increment;

#[message]
#[derive(PartialEq)]
struct Added(u32);

#[message(ret = u32)]
struct Summarize;

#[derive(Debug, Deserialize)]
struct Config {
    step: u32,
}

async fn summator(mut ctx: Context<Config>) {
    let mut sum = 0;

    while let Some(envelope) = ctx.recv().await {
        msg!(match envelope {
            Increment => {
                let step = ctx.config().step;
                sum += step;
                let _ = ctx.send(Added(step)).await;
            }
            (Summarize, token) => {
                ctx.respond(token, sum);
            }
        })
    }
}

pub fn summators() -> Blueprint {
    ActorGroup::new().config::<Config>().exec(summator)
}

#[tokio::test]
async fn it_works() {
    // Note: `RUST_LOG=elfo` can be provided to see all messages in failed cases.

    // Define a config (usually using `toml!` or `json!`).
    let config = toml::toml! {
        step = 20
    };

    // ... or provide the default one.
    let _config = elfo::config::AnyConfig::default();

    // Wrap the actor group to take control over it.
    let mut proxy = elfo::test::proxy(summators(), config).await;

    // How to send messages to the group.
    proxy.send(Increment).await;
    proxy.send(Increment).await;

    // It's possible to wait until the actor handles sent messages.
    // But usually it isn't required.
    proxy.sync().await;

    // How to check actors' output.
    assert_msg!(proxy.recv().await, Added(15u32..=35)); // Note: rhs is a pattern.
    assert_msg_eq!(proxy.recv().await, Added(20));

    // How to check request-response.
    assert_eq!(proxy.request(Summarize).await, 40);

    // By default, the proxy checks that there are no more unfetched messages
    // (in `impl Drop`). It's possible to alter this behavior.
    proxy.non_exhaustive();
}

#[tokio::test]
async fn it_uses_subproxies() {
    let config = toml::toml! { step = 20 };
    let mut proxy = elfo::test::proxy(summators(), config).await;

    // It's possible to get a subproxy with a different address.
    // The main purpose is to test `send_to(..)` and `request_to(..)` calls.
    // Subproxies inherit properties (`non_exhaustive` etc) from the original proxy.
    // Note that API is likely to be changed in the future.
    let mut subproxy = proxy.subproxy().await;
    assert_eq!(subproxy.request(Summarize).await, 0);
    assert!(proxy.try_recv().is_none());

    // `send(..)` and `request(..)` always send messages to the original proxy.
    subproxy.send(Increment).await;
    assert!(subproxy.try_recv().is_none());
    assert_msg_eq!(proxy.recv().await, Added(20));
}

fn main() {
    panic!("run `cargo test`");
}
