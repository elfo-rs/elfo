use std::{iter, panic, sync::Arc, time::Duration};

use eyre::{Result, WrapErr};
use fxhash::FxHashSet;
use parking_lot::Mutex;
use tokio::task;
use tracing::{error, info};

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    dumping::{self, INTERNAL_CLASS},
    group::TerminationPolicy,
    messages::{ConfigUpdated, Terminate, UpdateConfig},
    routers::{MapRouter, Outcome},
    scope::{self, SerdeMode},
    signal::{Signal, SignalKind},
    time::Interval,
    ActorGroup, Context, Schema,
};
use elfo_utils::ward;

use crate::{
    config::Config,
    dump_storage::{Drain, DumpRegistry, DumpStorage},
    file_registry::{FileHandle, FileRegistry},
    reporter::{Report, Reporter},
    rule_set::RuleSet,
    serializer::Serializer,
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

        let mut serializer = Serializer::new(self.dump_registry.class());
        let mut rule_set = RuleSet::new(self.dump_registry.class());
        let mut reporter = Reporter::default();
        let mut need_to_terminate = false;

        rule_set.configure(&self.ctx.config().rules);

        let signal = Signal::new(SignalKind::Hangup, || ReopenDumpFile);
        let interval = Interval::new(|| DumpingTick);
        // TODO: `interval.after` to set random time shift.
        interval.set_period(self.ctx.config().interval);

        let mut ctx = self.ctx.clone().with(&signal).with(&interval);

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                ConfigUpdated => {
                    let config = ctx.config();
                    interval.set_period(config.interval);

                    path = config.path(ctx.key());
                    self.file_registry
                        .open(&path, false)
                        .await
                        .wrap_err("cannot open the dump file")?;

                    rule_set.configure(&config.rules);

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

                    // A blocking background task that writes a lot of dumps in batch.
                    // It's much faster than calling tokio's async functions.
                    let background = move || -> Result<(Serializer, RuleSet, Reporter)> {
                        let mut report = Report::default();

                        let res = scope::with_serde_mode(SerdeMode::Dumping, || {
                            write_dumps(
                                dump_registry.drain(timeout),
                                &mut serializer,
                                &mut rule_set,
                                file,
                                &mut report,
                            )
                        });

                        reporter.add(report);

                        res?;
                        Ok((serializer, rule_set, reporter))
                    };

                    // Run the background task and wait until it's completed.
                    let scope = scope::expose();
                    match task::spawn_blocking(|| scope.sync_within(background)).await {
                        Ok(Ok(state)) => {
                            serializer = state.0;
                            rule_set = state.1;
                            reporter = state.2;
                        }
                        Ok(Err(err)) => return Err(err),
                        Err(err) => panic::resume_unwind(err.into_panic()),
                    }

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

        let classes = classes.clone();
        drop(dump_storage);

        info!("new classes are found, starting more dumpers");

        // Create more dumpers for new classes.
        for class in classes.difference(&m.known_classes) {
            let msg = StartDumperForClass(class.to_string());

            if let Err(err) = self.ctx.try_send_to(self.ctx.group(), msg) {
                error!(%class, error = %err, "cannot start a new dumper");
            }
        }

        m.known_classes = classes;
    }
}

fn write_dumps(
    dumps: Drain<'_>,
    serializer: &mut Serializer,
    rule_set: &mut RuleSet,
    file: FileHandle,
    report: &mut Report,
) -> Result<()> {
    for dump in dumps {
        let params = rule_set.get(dump.message_protocol, &dump.message_name);
        let (chunk, new_report) = ward!(serializer.append(&dump, params), continue);
        file.write(chunk).context("cannot write to the dump file")?;
        report.merge(new_report);
    }

    if let Some((chunk, new_report)) = serializer.take() {
        file.write(chunk).context("cannot write to the dump file")?;
        report.merge(new_report);
    }

    Ok(())
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
                StartDumperForClass(class) => Outcome::Unicast(class.clone()),
                _ => Outcome::Default,
            })
        }))
        .exec(move |ctx| Dumper::new(ctx, storage_1.clone(), file_registry.clone()).main())
}
