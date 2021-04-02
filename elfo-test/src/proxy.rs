use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant as StdInstant},
};

use futures_intrusive::channel::shared;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tokio::task;

use elfo_core::{
    ActorGroup, Context, Envelope, Message, Request, ResponseToken, Schema, Topology,
    _priv::do_start,
};

const MAX_WAIT_TIME: Duration = Duration::from_millis(100);

pub struct Proxy {
    context: Context,
    non_exhaustive: bool,
}

// TODO: add `#[track_caller]` after https://github.com/rust-lang/rust/issues/78840.
impl Proxy {
    pub async fn send<M: Message>(&self, message: M) {
        let res = self.context.send(message).await;
        res.expect("cannot send message")
    }

    pub async fn request<R: Request>(&self, request: R) -> R::Response {
        let res = self.context.request(request).resolve().await;
        res.expect("cannot send request")
    }

    pub fn respond<R: Request>(&self, token: ResponseToken<R>, response: R::Response) {
        self.context.respond(token, response)
    }

    pub async fn recv(&mut self) -> Envelope {
        // We are forced to use `std::time::Instant` instead of `tokio::time::Instant`
        // because we don't want to use mocked time by tokio here.
        let start = StdInstant::now();

        while {
            if let Some(envelope) = self.try_recv() {
                return envelope;
            }

            task::yield_now().await;
            start.elapsed() < MAX_WAIT_TIME
        } {}

        panic!("too long");
    }

    pub fn try_recv(&mut self) -> Option<Envelope> {
        self.context.try_recv().ok()
    }

    pub fn non_exhaustive(&mut self) {
        self.non_exhaustive = true;
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if !self.non_exhaustive {
            if let Some(envelope) = self.try_recv() {
                panic!(
                    "test ended, but not all messages has been consumed: {:?}",
                    envelope
                );
            }
        }
    }
}

fn testers(tx: shared::OneshotSender<Context>) -> Schema {
    let tx = Arc::new(tx);

    ActorGroup::new().exec(move |mut ctx| {
        let tx = tx.clone();
        async move {
            // Actually starts actor.
            let _ = ctx.recv().await;

            let _ = tx.send(ctx);
            futures::future::pending::<()>().await;
        }
    })
}

pub async fn proxy(schema: Schema, config: impl for<'de> Deserializer<'de>) -> Proxy {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Value::deserialize(config).expect("invalid config");
    let mut map = BTreeMap::new();
    map.insert(Value::String("subject".into()), config);
    let config = Value::Map(map);

    let topology = Topology::empty();
    let subject = topology.local("subject");
    let testers = topology.local("system.testers");
    let configurers = topology.local("system.configurers").entrypoint();

    testers.route_all_to(&subject);
    subject.route_all_to(&testers);

    // TODO: capture log messages.
    // TODO: capture metrics.
    configurers.mount(elfo_core::configurer::fixture(&topology, config));
    subject.mount(schema);

    let (tx, rx) = shared::oneshot_channel();
    testers.mount(self::testers(tx));

    do_start(topology).await.expect("cannot start");

    Proxy {
        context: rx.receive().await.expect("cannot receive tester's context"),
        non_exhaustive: false,
    }
}
