use std::{error::Error as StdError, fs::File, io::Write, iter, sync::Arc};

use eyre::{Result, WrapErr};
use fxhash::FxHashSet;
use metrics::counter;
use parking_lot::Mutex;
use tokio::{task, time::Duration};
use tracing::{error, info, warn};

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo_utils::cooldown;

use elfo::{
    dumping::{self, INTERNAL_CLASS},
    group::TerminationPolicy,
    messages::{ConfigUpdated, Terminate, UpdateConfig},
    routers::{MapRouter, Outcome},
    signal::{Signal, SignalKind},
    time::Interval,
    ActorGroup, Context, Schema,
};

use crate::{
    config::Config,
    dump_buffer::{AppendError, DumpBuffer},
    storage::Storage,
};

#[message(elfo = elfo_core)]
struct StartDumperForClass(String);

#[message(elfo = elfo_core)]
struct ReopenDumpFile;

#[message(elfo = elfo_core)]
struct DumpingTick;

struct Dumper {
    ctx: Context<Config, String>,
    storage: Arc<Mutex<Storage>>,

    // Used only by the master actor.
    known_classes: FxHashSet<&'static str>,
}

impl Dumper {
    fn new(ctx: Context<Config, String>, storage: Arc<Mutex<Storage>>) -> Self {
        Self {
            ctx,
            storage,
            known_classes: iter::once(INTERNAL_CLASS).collect(),
        }
    }

    fn is_manager(&self) -> bool {
        // Now we use a dumper for the `internal` class as a manager.
        self.ctx.key() == INTERNAL_CLASS
    }

    async fn main(mut self) -> Result<()> {
        let mut file = open_file(self.ctx.config(), self.ctx.key()).await;
        let mut buffer = DumpBuffer::new(self.ctx.config().max_dump_size);

        let class = Box::leak(self.ctx.key().clone().into_boxed_str());
        let registry = { self.storage.lock().registry(class) };

        let mut need_to_terminate = false;

        let signal = Signal::new(SignalKind::Hangup, || ReopenDumpFile);
        let interval = Interval::new(|| DumpingTick);
        // TODO: `interval.after` to set random time shift.
        interval.set_period(self.ctx.config().interval);

        let mut ctx = self.ctx.clone().with(&signal).with(&interval);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                // TODO: open on `ValidateConfig`
                ReopenDumpFile | ConfigUpdated => {
                    let config = ctx.config();
                    interval.set_period(config.interval);
                    file = open_file(config, self.ctx.key()).await;
                    buffer.configure(config.max_dump_size);

                    if self.is_manager() {
                        self.storage.lock().configure(config.registry_capacity);
                    }
                }
                DumpingTick => {
                    let registry = registry.clone();
                    let timeout = ctx.config().interval;

                    // TODO: run inside scope?
                    let report = task::spawn_blocking(move || -> Result<Report> {
                        let mut errors = Vec::new();
                        let mut written = 0;

                        dumping::set_in_dumping(true);

                        for dump in registry.drain(timeout) {
                            match buffer.append(&dump) {
                                Ok(buf) => {
                                    written += 1;

                                    if let Some(buf) = buf {
                                        file.write_all(buf).context("cannot write")?;
                                    }
                                }
                                Err(AppendError::LimitExceeded(_limit)) => {
                                    // TODO: add to separate `errors`.
                                }
                                Err(AppendError::SerializationFailed(err)) => {
                                    // TODO: avoid this hardcode.
                                    if errors.len() < 3
                                        && !errors
                                            .iter()
                                            .any(|(name, _)| name == &dump.message_name)
                                    {
                                        errors.push((dump.message_name, err));
                                    }
                                }
                            }
                        }

                        if let Some(buf) = buffer.take() {
                            file.write_all(buf).context("cannot write")?;
                        }

                        file.flush().context("cannot flush")?;
                        dumping::set_in_dumping(false);

                        Ok(Report {
                            file,
                            buffer,
                            errors,
                            written,
                        })
                    })
                    .await
                    .expect("failed to dump")?;

                    counter!("elfo_written_dumps_total", report.written);
                    file = report.file;
                    buffer = report.buffer;

                    // TODO: add a metrics for failed dumps.
                    cooldown!(Duration::from_secs(15), {
                        for (message_name, error) in report.errors {
                            warn!(
                                message = message_name,
                                error = &error as &(dyn StdError),
                                "cannot serialize"
                            );
                        }
                    });

                    if need_to_terminate {
                        break;
                    }

                    if self.is_manager() {
                        self.spawn_dumpers_if_needed();
                    }
                }
                Terminate => {
                    // TODO: use phases instead of a hardcoded delay.
                    interval.set_period(Duration::from_millis(250));
                    need_to_terminate = true;
                }
            });
        }

        info!("flushing the buffer and synchronizing the file");
        file.flush().context("cannot flush")?;
        file.sync_all().context("cannot sync")?;

        Ok(())
    }

    fn spawn_dumpers_if_needed(&mut self) {
        let storage = self.storage.lock();
        let classes = storage.classes();

        // Classes cannot be removed if created.
        if classes.len() == self.known_classes.len() {
            return;
        }

        info!("new classes are found, starting more dumpers");

        // Create more dumpers for new classes.
        for class in self.known_classes.difference(classes) {
            let msg = StartDumperForClass(class.to_string());

            if let Err(err) = self.ctx.try_send_to(self.ctx.group(), msg) {
                error!(%class, error = %err, "cannot start a new dumper");
            }
        }

        self.known_classes = classes.clone();
    }
}

struct Report {
    file: File,
    buffer: DumpBuffer,
    errors: Vec<(&'static str, serde_json::Error)>,
    written: u64,
}

fn collect_classes(map: &FxHashSet<&'static str>) -> Vec<String> {
    map.iter().map(|s| s.to_string()).collect()
}

pub(crate) fn new(storage: Arc<Mutex<Storage>>) -> Schema {
    let storage_1 = storage.clone();

    ActorGroup::new()
        .config::<Config>()
        .termination_policy(TerminationPolicy::manually())
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                // TODO: there is a rare race condition here,
                //       use `Broadcast & Unicast(INTERNAL_CLASS)` instead.
                UpdateConfig => Outcome::Multicast(collect_classes(storage.lock().classes())),
                _ => Outcome::Default,
            })
        }))
        .exec(move |ctx| Dumper::new(ctx, storage_1.clone()).main())
}

async fn open_file(config: &Config, class: &str) -> File {
    use tokio::fs::OpenOptions;

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.path(class))
        .await
        .expect("cannot open the dump file")
        .into_std()
        .await
}
