use std::sync::Arc;

use metrics::gauge;
use tokio::task::JoinHandle;
use tracing::{error, info};

use elfo_core as elfo;
use elfo_macros::{message, msg_raw as msg};

use elfo::{
    messages::ConfigUpdated, scope, time::Interval, tracing::TraceId, ActorGroup, Context,
    MoveOwnership, Schema,
};

use crate::{
    config::{Config, Retention, Sink},
    protocol::{GetSnapshot, Snapshot},
    render::Renderer,
    storage::Storage,
};

struct Telemeter {
    ctx: Context<Config>,
    interval: Interval<CompactionTick>,
    storage: Arc<Storage>,
    snapshot: Arc<Snapshot>,
    renderer: Renderer,
}

#[message(ret = Rendered, elfo = elfo_core)]
struct Render;

#[message(elfo = elfo_core)]
struct Rendered(#[serde(serialize_with = "elfo::dumping::hide")] String);

#[message(elfo = elfo_core)]
struct CompactionTick;

#[message(elfo = elfo_core)]
struct ServerFailed(MoveOwnership<hyper::Error>);

pub(crate) fn new(storage: Arc<Storage>) -> Schema {
    ActorGroup::new()
        .config::<Config>()
        .exec(move |ctx| Telemeter::new(ctx, storage.clone()).main())
}

impl Telemeter {
    pub(crate) fn new(mut ctx: Context<Config>, storage: Arc<Storage>) -> Self {
        let mut renderer = Renderer::default();
        renderer.configure(ctx.config());

        Self {
            interval: ctx.attach(Interval::new(CompactionTick)),
            storage,
            snapshot: Default::default(),
            renderer,
            ctx,
        }
    }

    async fn main(mut self) {
        // Now only prometheus is supported.
        assert_eq!(self.ctx.config().sink, Sink::Prometheus);

        let mut address = self.ctx.config().address;
        let mut server = start_server(&self.ctx);

        self.interval.start(self.ctx.config().compaction_interval);

        while let Some(envelope) = self.ctx.recv().await {
            msg!(match envelope {
                ConfigUpdated => {
                    let config = self.ctx.config();

                    if config.address != address {
                        info!("address changed, rerun the server");
                        server.abort();
                        address = config.address;
                        server = start_server(&self.ctx);
                    }

                    self.renderer.configure(config);
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
                ServerFailed(error) => {
                    error!(error = %&error.take().unwrap(), "server failed");
                    panic!("server failed");
                }
            });
        }

        server.abort();
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
}

fn start_server(ctx: &Context<Config>) -> JoinHandle<()> {
    use hyper::{
        server::{conn::AddrStream, Server},
        service::{make_service_fn, service_fn},
        Body, Error as HyperError, Response,
    };

    let address = ctx.config().address;
    let ctx = Arc::new(ctx.pruned());
    let ctx1 = ctx.clone();

    let scope = scope::expose();
    let scope1 = scope.clone();

    let serving = async move {
        let server = Server::try_bind(&address)?;
        let make_svc = make_service_fn(move |_socket: &AddrStream| {
            let ctx = ctx.clone();
            let scope = scope.clone();

            async move {
                Ok::<_, HyperError>(service_fn(move |_| {
                    let ctx = ctx.clone();
                    let scope = scope.clone();

                    let f = async move {
                        let Rendered(output) = ctx
                            .request_to(ctx.addr(), Render)
                            .resolve()
                            .await
                            .expect("failed to send to the telemeter");

                        Ok::<_, HyperError>(Response::new(Body::from(output)))
                    };

                    scope.set_trace_id(TraceId::generate());
                    scope.within(f)
                }))
            }
        });
        server.serve(make_svc).await
    };

    tokio::spawn(async move {
        if let Err(err) = serving.await {
            let f = async {
                let _ = ctx1.send(ServerFailed(err.into())).await;
            };

            scope1.set_trace_id(TraceId::generate());
            scope1.within(f).await;
        }
    })
}
