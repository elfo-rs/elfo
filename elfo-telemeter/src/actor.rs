use std::{sync::Arc, time::Duration};

use metrics::gauge;
use tracing::{error, info};

use elfo_core::{
    message, messages::ConfigUpdated, msg, stream::Stream, time::Interval, ActorGroup, Blueprint,
    Context, RestartParams, RestartPolicy, SourceHandle,
};

use crate::{
    config::{Config, Retention, Sink},
    hyper,
    protocol::{GetSnapshot, Render, Rendered, ServerFailed, Snapshot},
    render::Renderer,
    storage::Storage,
};

struct Telemeter {
    ctx: Context<Config>,
    interval: Interval<CompactionTick>,
    server: Option<Stream<ServerFailed>>,
    storage: Arc<Storage>,
    snapshot: Arc<Snapshot>,
    renderer: Renderer,
}

#[message]
struct CompactionTick;

pub(crate) fn new(storage: Arc<Storage>) -> Blueprint {
    ActorGroup::new()
        .config::<Config>()
        .restart_policy(RestartPolicy::on_failure(RestartParams::new(
            Duration::from_secs(5),
            Duration::from_secs(30),
        )))
        .stop_order(100)
        .exec(move |ctx| Telemeter::new(ctx, storage.clone()).main())
}

impl Telemeter {
    pub(crate) fn new(mut ctx: Context<Config>, storage: Arc<Storage>) -> Self {
        let mut renderer = Renderer::default();
        renderer.configure(ctx.config());

        Self {
            interval: ctx.attach(Interval::new(CompactionTick)),
            server: None,
            storage,
            snapshot: Default::default(),
            renderer,
            ctx,
        }
    }

    async fn main(mut self) {
        // Now only prometheus is supported.
        assert_eq!(self.ctx.config().sink, Sink::Prometheus);

        let mut listen = self.ctx.config().listen;
        self.start_server();

        self.interval.start(self.ctx.config().compaction_interval);

        while let Some(envelope) = self.ctx.recv().await {
            msg!(match envelope {
                ConfigUpdated => {
                    let config = self.ctx.config();

                    self.renderer.configure(config);

                    if config.listen != listen {
                        info!(
                            message = "listen address changed, rerun the server",
                            old = %listen,
                            new = %config.listen,
                        );
                        listen = config.listen;
                        self.start_server();
                    }
                }
                (GetSnapshot, token) => {
                    // Rendering includes compaction, skip extra compaction tick.
                    self.interval.start(self.ctx.config().compaction_interval);

                    self.fill_snapshot(/* only_histograms = */ false);
                    self.ctx.respond(token, self.snapshot.clone().into());
                }
                (Render, token) => {
                    // Rendering includes compaction, skip extra compaction tick.
                    self.interval.start(self.ctx.config().compaction_interval);

                    self.fill_snapshot(/* only_histograms = */ false);
                    let descriptions = self.storage.descriptions();
                    let output = self.renderer.render(&self.snapshot, &descriptions);
                    drop(descriptions);

                    self.ctx.respond(token, Rendered(output));

                    if self.ctx.config().retention == Retention::ResetOnScrape {
                        self.reset_distributions();
                    }
                }
                CompactionTick => {
                    self.fill_snapshot(/* only_histograms = */ true);
                }
                ServerFailed(err) => {
                    error!(error = %err, "server failed");
                    panic!("server failed, cannot continue");
                }
            });
        }
    }

    fn fill_snapshot(&mut self, only_histograms: bool) {
        // Reuse the latest snapshot if possible.
        let snapshot = Arc::make_mut(&mut self.snapshot);
        let size = self.storage.fill_snapshot(snapshot, only_histograms);

        if !only_histograms {
            gauge!("elfo_metrics_usage_bytes", size as f64);
        }
    }

    fn reset_distributions(&mut self) {
        // Reuse the latest snapshot if possible.
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.distributions_mut().for_each(|d| d.reset());
    }

    fn start_server(&mut self) {
        // Terminate a running server.
        if let Some(source) = self.server.take() {
            source.terminate();
        }

        // Start a new one.
        let listen = self.ctx.config().listen;
        let pruned_ctx = self.ctx.pruned();
        let source = Stream::once(hyper::server(listen, pruned_ctx));

        self.server = Some(self.ctx.attach(source));
    }
}
