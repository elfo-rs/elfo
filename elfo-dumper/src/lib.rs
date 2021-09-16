#![warn(rust_2018_idioms, unreachable_pub)]

use std::{
    error::Error as StdError,
    fs::File,
    io::{BufWriter, Write},
};

use eyre::{Result, WrapErr};
use metrics::counter;
use tokio::{task, time};
use tracing::error;

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    ActorGroup, Context, Schema,
    _priv::dumping,
    group::TerminationPolicy,
    messages::{ConfigUpdated, Terminate},
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

    async fn main(self) -> Result<()> {
        let mut file = open_file(self.ctx.config()).await;

        // TODO: use the grant system instead.
        let dumper = dumping::of(&self.ctx);
        let mut need_to_terminate = false;

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
                    let timeout = ctx.config().interval;

                    let report = task::spawn_blocking(move || -> Result<Report> {
                        let mut errors = Vec::new();
                        let mut written = 0;

                        for dump in dumper.drain(timeout) {
                            match serde_json::to_writer(&mut file, &dump) {
                                Ok(()) => {
                                    written += 1;
                                }
                                Err(err) if err.is_io() => {
                                    Err(err).context("cannot write")?;
                                }
                                Err(err) => {
                                    errors.push((dump.message_name, err));
                                    // TODO: the last line is probably invalid,
                                    //       should we use custom buffer?
                                }
                            };
                            file.write_all(b"\n").context("cannot write")?;
                        }
                        file.flush().context("cannot flush")?;

                        Ok(Report {
                            file,
                            errors,
                            written,
                        })
                    })
                    .await
                    .expect("failed to dump")?;

                    counter!("elfo_written_dumps_total", report.written);
                    file = report.file;

                    // TODO: add a metrics for failed dumps.
                    for (message_name, error) in report.errors {
                        error!(
                            message = message_name,
                            error = &error as &(dyn StdError),
                            "cannot serialize"
                        );
                    }

                    if need_to_terminate {
                        break;
                    }
                }
                Terminate => {
                    // TODO: use phases instead of hardcoded delay.
                    interval.set_period(time::Duration::from_millis(250));
                    need_to_terminate = true;
                }
            });
        }

        file.flush().context("cannot flush")?;
        file.get_ref().sync_all().context("cannot sync")?;

        Ok(())
    }
}

struct Report {
    file: BufWriter<File>,
    errors: Vec<(&'static str, serde_json::Error)>,
    written: u64,
}

pub fn new() -> Schema {
    ActorGroup::new()
        .config::<Config>()
        .termination_policy(TerminationPolicy::manually())
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
