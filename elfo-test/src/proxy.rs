use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant as StdInstant},
};

use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tokio::task;

use elfo_core::{
    self as elfo, ActorGroup, Addr, Context, Envelope, Local, Message, Request, ResponseToken,
    Schema,
    _priv::do_start,
    routers::{MapRouter, Outcome},
    topology::{GetAddrs, Topology},
};
use elfo_macros::{message, msg_raw as msg};

const MAX_WAIT_TIME: Duration = Duration::from_millis(100);

pub struct Proxy {
    context: Context,
    non_exhaustive: bool,
    testers_addr: Addr,
}

// TODO: add `#[track_caller]` after https://github.com/rust-lang/rust/issues/78840.
impl Proxy {
    pub async fn send<M: Message>(&self, message: M) {
        let res = self.context.send(message).await;
        res.expect("cannot send message")
    }

    pub async fn send_to<M: Message>(&self, recipient: Addr, message: M) {
        let res = self.context.send_to(recipient, message).await;
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

    #[deprecated]
    pub fn set_addr(&mut self, addr: Addr) {
        #[allow(deprecated)]
        self.context.set_addr(addr);
    }

    pub fn non_exhaustive(&mut self) {
        self.non_exhaustive = true;
    }

    /// Creates a subproxy with a different address.
    /// The main purpose is to test `send_to(..)` and `request(..).from(..)`
    /// calls. It's likely to be changed in the future.
    pub async fn subproxy(&self) -> Proxy {
        Proxy {
            context: self
                .context
                .request(StealContext)
                .from(self.testers_addr)
                .resolve()
                .await
                .expect("cannot steal tester's context")
                .into_inner(),
            non_exhaustive: self.non_exhaustive,
            testers_addr: self.testers_addr,
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if !self.non_exhaustive && !thread::panicking() {
            if let Some(envelope) = self.try_recv() {
                panic!(
                    "test ended, but not all messages has been consumed: {:?}",
                    envelope
                );
            }
        }
    }
}

#[message(ret = Local<Context>, elfo = elfo_core)]
struct StealContext;

fn testers() -> Schema {
    let next_tester_key = AtomicUsize::new(0);

    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                StealContext => Outcome::Unicast(next_tester_key.fetch_add(1, Ordering::SeqCst)),
                _ => Outcome::Unicast(0),
            })
        }))
        .exec(move |mut ctx| async move {
            while let Some(envelope) = ctx.recv().await {
                msg!(match envelope {
                    (StealContext, token) => {
                        ctx.respond(token, Local::from(ctx.pruned()));
                        futures::future::pending::<()>().await;
                    }
                });
            }
        })
}

pub async fn proxy(schema: Schema, config: impl for<'de> Deserializer<'de>) -> Proxy {
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

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
    configurers.mount(elfo_configurer::fixture(&topology, config));
    subject.mount(schema);

    assert_eq!(testers.addrs().len(), 1);
    let testers_addr = testers.addrs()[0];
    testers.mount(self::testers());

    let root_ctx = do_start(topology).await.expect("cannot start");

    Proxy {
        context: root_ctx
            .request(StealContext)
            .from(testers_addr)
            .resolve()
            .await
            .expect("cannot steal tester's context")
            .into_inner(),
        non_exhaustive: false,
        testers_addr,
    }
}
