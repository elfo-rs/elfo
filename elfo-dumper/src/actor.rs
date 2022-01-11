use std::{error::Error as StdError, iter, sync::Arc};

use eyre::{Result, WrapErr};
use fxhash::FxHashSet;
use metrics::counter;
use parking_lot::Mutex;
use tokio::{task, time::Duration};
use tracing::{error, info, warn};

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo_utils::{cooldown, ward};

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
    dump_storage::{DumpRegistry, DumpStorage},
    file_registry::FileRegistry,
};

#[message(elfo = elfo_core)]
struct StartDumperForClass(String);

#[message(elfo = elfo_core)]
struct ReopenDumpFile;

#[message(elfo = elfo_core)]
struct DumpingTick;

struct Dumper {
    ctx: Context<Config, String>,
    dump_registry: Arc<DumpRegistry>,
    file_registry: Arc<FileRegistry>,

    // Used only by the manager actor.
    manager: Option<Manager>,
}

struct Manager {
    dump_storage: Arc<Mutex<DumpStorage>>,
    known_classes: FxHashSet<&'static str>,
}

impl Dumper {
    fn new(
        ctx: Context<Config, String>,
        dump_storage: Arc<Mutex<DumpStorage>>,
        file_registry: Arc<FileRegistry>,
    ) -> Self {
        // TODO: avoid leaking here.
        let class = Box::leak(ctx.key().clone().into_boxed_str());
        let dump_registry = dump_storage.lock().registry(class);

        let manager = if ctx.key() == INTERNAL_CLASS {
            Some(Manager {
                dump_storage,
                known_classes: iter::once(INTERNAL_CLASS).collect(),
            })
        } else {
            None
        };

        Self {
            ctx,
            dump_registry,
            file_registry,
            manager,
        }
    }

    async fn main(mut self) -> Result<()> {
        let mut path = self.ctx.config().path(self.ctx.key());
        self.file_registry
            .open(&path, false)
            .await
            .wrap_err("cannot open the dump file")?;

        let mut buffer = DumpBuffer::new(self.ctx.config().max_dump_size);
        let mut need_to_terminate = false;

        let signal = Signal::new(SignalKind::Hangup, || ReopenDumpFile);
        let interval = Interval::new(|| DumpingTick);
        // TODO: `interval.after` to set random time shift.
        interval.set_period(self.ctx.config().interval);

        let mut ctx = self.ctx.clone().with(&signal).with(&interval);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                // TODO: check the file on `ValidateConfig`
                ConfigUpdated => {
                    let config = ctx.config();
                    interval.set_period(config.interval);
                    buffer.configure(config.max_dump_size);

                    path = config.path(ctx.key());
                    self.file_registry
                        .open(&path, false)
                        .await
                        .wrap_err("cannot open the dump file")?;

                    if let Some(m) = &self.manager {
                        m.dump_storage.lock().configure(config.registry_capacity);
                    }
                }
                ReopenDumpFile => {
                    // TODO: reopen the dump file at most once.
                    // It's possible to reopen the file multiple times,
                    // if the same file is used for multiple classes.
                    // It's ok for now, but should be fixed later.
                    self.file_registry
                        .open(&path, true)
                        .await
                        .wrap_err("cannot reopen the dump file")?;
                }
                DumpingTick => {
                    let timeout = ctx.config().interval;
                    let dump_registry = self.dump_registry.clone();
                    let file = self.file_registry.acquire(&path).await;

                    // TODO: run inside scope?
                    let report = task::spawn_blocking(move || -> Result<Report> {
                        let mut errors = Vec::new();
                        let mut written = 0;

                        dumping::set_in_dumping(true);

                        for dump in dump_registry.drain(timeout) {
                            match buffer.append(&dump) {
                                Ok(buf) => {
                                    written += 1;

                                    if let Some(buf) = buf {
                                        file.write(buf).context("cannot write to the dump file")?;
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
                            file.write(buf).context("cannot write to the dump file")?;
                        }

                        dumping::set_in_dumping(false);

                        Ok(Report {
                            buffer,
                            errors,
                            written,
                        })
                    })
                    .await
                    .expect("failed to dump")?;

                    counter!("elfo_written_dumps_total", report.written);
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

                    self.spawn_dumpers_if_needed();
                }
                Terminate => {
                    // TODO: use phases instead of a hardcoded delay.
                    interval.set_period(Duration::from_millis(250));
                    need_to_terminate = true;
                }
            });
        }

        info!("synchronizing the file");
        self.file_registry
            .sync(&path)
            .await
            .context("cannot sync the dump file")?;

        Ok(())
    }

    fn spawn_dumpers_if_needed(&mut self) {
        let m = ward!(self.manager.as_mut());

        let dump_storage = m.dump_storage.lock();
        let classes = dump_storage.classes();

        // Classes cannot be removed if created.
        if classes.len() == m.known_classes.len() {
            return;
        }

        info!("new classes are found, starting more dumpers");

        // Create more dumpers for new classes.
        for class in m.known_classes.difference(classes) {
            let msg = StartDumperForClass(class.to_string());

            if let Err(err) = self.ctx.try_send_to(self.ctx.group(), msg) {
                error!(%class, error = %err, "cannot start a new dumper");
            }
        }

        m.known_classes = classes.clone();
    }
}

struct Report {
    buffer: DumpBuffer,
    errors: Vec<(&'static str, serde_json::Error)>,
    written: u64,
}

fn collect_classes(map: &FxHashSet<&'static str>) -> Vec<String> {
    map.iter().map(|s| s.to_string()).collect()
}

pub(crate) fn new(dump_storage: Arc<Mutex<DumpStorage>>) -> Schema {
    let storage_1 = dump_storage.clone();
    let file_registry = Arc::new(FileRegistry::default());

    ActorGroup::new()
        .config::<Config>()
        .termination_policy(TerminationPolicy::manually())
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                // TODO: there is a rare race condition here,
                //       use `Broadcast & Unicast(INTERNAL_CLASS)` instead.
                UpdateConfig => Outcome::Multicast(collect_classes(dump_storage.lock().classes())),
                _ => Outcome::Default,
            })
        }))
        .exec(move |ctx| Dumper::new(ctx, storage_1.clone(), file_registry.clone()).main())
}
