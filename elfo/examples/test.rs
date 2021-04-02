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

pub fn summators() -> Schema {
    ActorGroup::new().config::<Config>().exec(summator)
}

#[tokio::test]
async fn it_works() {
    // Fistly, you can provide `RUST_LOG=elfo` to see all messages for failed cases.

    // Define a config (usually using `toml!` or `json!`).
    let config = toml::toml! {
        step = 20
    };

    // Wrap the actor group to take control over it.
    let mut proxy = elfo::test::proxy(summators(), config).await;

    // How to send messages to the group.
    proxy.send(Increment).await;
    proxy.send(Increment).await;

    // How to check actors' output.
    assert_msg!(proxy.recv().await, Added(15u32..=35)); // Note: rhs is a pattern.
    assert_msg_eq!(proxy.recv().await, Added(20));

    // How to check request-response.
    assert_eq!(proxy.request(Summarize).await, 40);

    // By default, the proxy checks that there are no more unfetched messages
    // (in `impl Drop`). It's possible to alter this behavior.
    proxy.non_exhaustive();
}

fn main() {
    panic!("run `cargo test`");
}
