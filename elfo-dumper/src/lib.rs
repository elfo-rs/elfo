#![warn(rust_2018_idioms, unreachable_pub)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use futures::{future, FutureExt};
use fxhash::FxHashMap;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tracing::error;

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    config::AnyConfig,
    errors::RequestError,
    messages::{ConfigRejected, Ping, UpdateConfig, ValidateConfig},
    signal::{Signal, SignalKind},
    time::Interval,
    ActorGroup, ActorStatus, Addr, Context, Request, Schema, Topology,
};

#[message(elfo = elfo_core)]
pub struct ReopenDumpFile;

#[message(elfo = elfo_core)]
struct DumpingTick;

struct Dumper {
    ctx: Context,
}

impl Dumper {
    fn new(ctx: Context) -> Self {
        Self { ctx }
    }

    async fn main(mut self) {
        let signal = Signal::new(SignalKind::Hangup, || ReopenDumpFile);
        let interval = Interval::new(|| DumpingTick);

        let mut ctx = self.ctx.clone().with(&signal).with(&interval);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                ReopenDumpFile => {
                    // TODO
                }
                DumpingTick => {
                    // TODO
                }
            });
        }
    }
}
