use elfo::{
    messages::UpdateConfig,
    prelude::*,
    routers::{MapRouter, Outcome},
};

use crate::protocol::*;

// For simplicity, we use strategy name as a sharding key.
// In a real system, it's more likely to be some kind + name pair or even ID.
type StrategyName = String;

#[derive(Debug, serde::Deserialize)]
struct Config {
    predefined: Vec<StrategyName>,
}

pub fn new() -> Blueprint {
    ActorGroup::new()
        .config::<Config>()
        // Actors in this group are started at startup or when config is updated.
        // It's obviously not the only way and depends on the system requirements.
        // For instance, you can start on some `StartStrategy` message from
        // actors implementing GUI/WUI/CLI communication or something else.
        .router(MapRouter::with_state(
            |config: &Config, _| config.predefined.clone(),
            |envelope, strategies| {
                msg!(match envelope {
                    UpdateConfig => Outcome::Multicast(strategies.clone()),
                    _ => Outcome::Default,
                })
            },
        ))
        .exec(|ctx| async move {
            // Hardcoded list of strategies.
            // In a real system, this would be done using some factory outside of the actor
            // code to separate the common logic from the specific strategies.
            // Such implementations can be "installed" during building (linkme/inventory
            // crates) or even loaded dynamically (if your strategy SDK provides
            // stable ABI, stabby crate). So, this is just a demo.
            match ctx.key().as_str() {
                "strategy_a" => exec::<StrategyA>(ctx).await,
                "strategy_b" => exec::<StrategyB>(ctx).await,
                name => tracing::error!("unknown strategy: {name}"),
            }
        })
}

async fn exec<S: Strategy>(mut ctx: Context<Config, StrategyName>) {
    let mut strategy = S::default();

    // Subscribe to all required markets.
    for market in strategy.required_md() {
        let response = ctx
            .request(SubscribeToMd { market })
            .resolve()
            .await
            .unwrap();

        strategy.on_md_snap(response.market, &response.snapshot);
    }

    while let Some(envelope) = ctx.recv().await {
        msg!(match envelope {
            MdUpdated { market, levels } => strategy.on_md_inc(market, &levels),
        });

        // Run a cycle of the strategy on every update.
        // Not very efficient, but good enough for a demo.
        strategy.exec();
    }
}

// === Strategies ===
//
// In a real system, all strategies would be in separate crates (probably even
// separate repos).

trait Strategy: Default {
    fn required_md(&self) -> Vec<Market>;
    fn on_md_snap(&mut self, market: Market, snap: &[MdLevel]);
    fn on_md_inc(&mut self, market: Market, inc: &[MdLevel]);
    fn exec(&mut self);
}

#[derive(Default)]
struct StrategyA {}

impl Strategy for StrategyA {
    fn required_md(&self) -> Vec<Market> {
        vec!["a".into()]
    }

    fn on_md_snap(&mut self, market: Market, snap: &[MdLevel]) {
        // Log for example purposes. In a real system, the dumping covers it.
        tracing::info!("got snapshot for {market}: {snap:?}");
    }

    fn on_md_inc(&mut self, market: Market, inc: &[MdLevel]) {
        tracing::info!("got incremental update for {market}: {inc:?}");
    }

    fn exec(&mut self) {}
}

#[derive(Default)]
struct StrategyB {}

impl Strategy for StrategyB {
    fn required_md(&self) -> Vec<Market> {
        vec!["a".into(), "b".into()]
    }

    fn on_md_snap(&mut self, market: Market, snap: &[MdLevel]) {
        tracing::info!("got snapshot for {market}: {snap:?}");
    }

    fn on_md_inc(&mut self, market: Market, inc: &[MdLevel]) {
        tracing::info!("got incremental update for {market}: {inc:?}");
    }

    fn exec(&mut self) {}
}
