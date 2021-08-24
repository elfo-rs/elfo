#![warn(rust_2018_idioms, unreachable_pub)]

use std::{
    fs::File,
    io::{BufWriter, Write},
};

use tokio::task;

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    ActorGroup, Context, Schema,
    _priv::dumping,
    messages::ConfigUpdated,
    signal::{Signal, SignalKind},
    time::Interval,
};

use self::config::Config;

mod config;

const BUFFER_CAPACITY: usize = 128 * 1024;

#[message(elfo = elfo_core)]
pub struct ReopenDumpFile;

#[message(elfo = elfo_core)]
struct DumpingTick;

struct Dumper {
    ctx: Context<Config>,
}

impl Dumper {
    fn new(ctx: Context<Config>) -> Self {
        Self { ctx }
    }

    async fn main(self) {
        let mut file = open_file(self.ctx.config()).await;
        let dumper = dumping::of(&self.ctx);

        let signal = Signal::new(SignalKind::Hangup, || ReopenDumpFile);
        let interval = Interval::new(|| DumpingTick);
        interval.set_period(self.ctx.config().interval);

        let mut ctx = self.ctx.clone().with(&signal).with(&interval);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                ReopenDumpFile | ConfigUpdated => {
                    let config = self.ctx.config();
                    interval.set_period(config.interval);
                    file = open_file(config).await;
                }
                DumpingTick => {
                    let dumper = dumper.clone();

                    // TODO: change error handling?
                    file = task::spawn_blocking(move || {
                        for dump in dumper.drain() {
                            serde_json::to_writer(&mut file, &dump).expect("cannot write");
                            file.write_all(b"\n").expect("cannot write");
                        }
                        file.flush().expect("cannot flush");
                        file
                    })
                    .await
                    .expect("failed to dump");
                }
            });
        }
    }
}

pub fn new() -> Schema {
    ActorGroup::new()
        .config::<Config>()
        .exec(|ctx| Dumper::new(ctx).main())
}

async fn open_file(config: &Config) -> BufWriter<File> {
    use tokio::fs::OpenOptions;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.path)
        .await
        .expect("cannot open the dump file")
        .into_std()
        .await;

    BufWriter::with_capacity(BUFFER_CAPACITY, file)
}
