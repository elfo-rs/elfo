use std::{
    collections::BTreeMap,
    future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant as StdInstant},
};

use futures_intrusive::channel::shared;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tokio::task;

use elfo_core::{
    self as elfo, ActorGroup, ActorMeta, Addr, Context, Envelope, Local, Message, Request,
    ResponseToken, Schema,
    _priv::do_start,
    routers::{MapRouter, Outcome},
    scope::Scope,
    topology::{GetAddrs, Topology},
};
use elfo_macros::{message, msg_raw as msg};

const MAX_WAIT_TIME: Duration = Duration::from_millis(150);
const SYNC_YIELD_COUNT: usize = 32;

pub struct Proxy {
    context: Context,
    scope: Scope,
    non_exhaustive: bool,
    subject_addr: Addr,
}

// TODO: add `#[track_caller]` after https://github.com/rust-lang/rust/issues/78840.
impl Proxy {
    pub fn addr(&self) -> Addr {
        self.context.addr()
    }

    pub async fn send<M: Message>(&self, message: M) {
        let f = async {
            let res = self.context.send(message).await;
            res.expect("cannot send message")
        };
        self.scope.clone().within(f).await
    }

    pub async fn send_to<M: Message>(&self, recipient: Addr, message: M) {
        let f = async {
            let res = self.context.send_to(recipient, message).await;
            res.expect("cannot send message")
        };
        self.scope.clone().within(f).await
    }

    pub async fn request<R: Request>(&self, request: R) -> R::Response {
        let f = async {
            let res = self.context.request(request).resolve().await;
            res.expect("cannot send message")
        };
        self.scope.clone().within(f).await
    }

    pub fn respond<R: Request>(&self, token: ResponseToken<R>, response: R::Response) {
        self.scope
            .clone()
            .sync_within(|| self.context.respond(token, response))
    }

    pub async fn recv(&mut self) -> Envelope {
        let scope = self.scope.clone();
        let f = async {
            // We are forced to use `std::time::Instant` instead of `tokio::time::Instant`
            // because we don't want to use mocked time by tokio here.
            let start = StdInstant::now();

            #[allow(clippy::blocks_in_if_conditions)]
            while {
                if let Some(envelope) = self.try_recv() {
                    return envelope;
                }

                task::yield_now().await;
                start.elapsed() < MAX_WAIT_TIME
            } {}

            panic!("too long");
        };
        scope.within(f).await
    }

    pub fn try_recv(&mut self) -> Option<Envelope> {
        self.scope
            .clone()
            .sync_within(|| self.context.try_recv().ok())
    }

    /// Waits until the testable actor handles all previously sent messages.
    ///
    /// Now it's implemented as multiple calls `yield_now()`,
    /// but the implementation can be changed in the future.
    pub async fn sync(&mut self) {
        for _ in 0..SYNC_YIELD_COUNT {
            task::yield_now().await;
        }
    }

    #[deprecated]
    pub fn set_addr(&mut self, addr: Addr) {
        #[allow(deprecated)]
        self.scope
            .clone()
            .sync_within(|| self.context.set_addr(addr))
    }

    pub fn non_exhaustive(&mut self) {
        self.non_exhaustive = true;
    }

    /// Creates a subproxy with a different address.
    /// The main purpose is to test `send_to(..)` and `request(..).from(..)`
    /// calls. It's likely to be changed in the future.
    pub async fn subproxy(&self) -> Proxy {
        let f = async {
            self.context
                .request_to(self.context.group(), StealContext)
                .resolve()
                .await
                .expect("cannot steal tester's context")
                .into_inner()
        };
        let context = self.scope.clone().within(f).await;

        let meta = Arc::new(ActorMeta {
            group: "subproxy".into(),
            key: String::new(),
        });

        Proxy {
            scope: Scope::test(context.addr(), meta),
            context,
            non_exhaustive: self.non_exhaustive,
            subject_addr: self.subject_addr,
        }
    }

    pub async fn finished(&self) {
        let fut = self.context.finished(self.subject_addr);
        self.scope.clone().within(fut).await
    }

    /// Closes a mailbox of the proxy.
    pub fn close(&self) {
        self.scope.clone().sync_within(|| self.context.close());
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if !self.non_exhaustive && !thread::panicking() {
            self.scope.clone().sync_within(|| {
                if let Some(envelope) = self.try_recv() {
                    panic!(
                        "test ended, but not all messages have been consumed: {:?}",
                        envelope
                    );
                }
            });
        }
    }
}

#[message(ret = Local<Context>, elfo = elfo_core)]
struct StealContext;

fn testers(tx: shared::OneshotSender<Context>) -> Schema {
    let tx = Arc::new(tx);
    let next_tester_key = AtomicUsize::new(1);

    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                StealContext => Outcome::Unicast(next_tester_key.fetch_add(1, Ordering::SeqCst)),
                _ => Outcome::Unicast(0),
            })
        }))
        .exec(move |mut ctx| {
            let tx = tx.clone();

            async move {
                if *ctx.key() == 0 {
                    let _ = tx.send(ctx.pruned());
                } else {
                    let envelope = ctx.recv().await.unwrap();
                    msg!(match envelope {
                        (StealContext, token) => {
                            ctx.respond(token, Local::from(ctx.pruned()));
                        }
                        envelope => panic!("unexpected message: {:?}", envelope),
                    });
                }

                future::pending::<()>().await;
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

    let subject_addr = subject.addrs()[0];

    testers.route_all_to(&subject);
    subject.route_all_to(&testers);

    // TODO: capture log messages.
    // TODO: capture metrics.
    configurers.mount(elfo_configurer::fixture(&topology, config));
    subject.mount(schema);

    let (tx, rx) = shared::oneshot_channel();
    testers.mount(self::testers(tx));
    do_start(topology, |_, _| future::ready(()))
        .await
        .expect("cannot start");

    let context = rx.receive().await.unwrap();
    let meta = Arc::new(ActorMeta {
        group: "proxy".into(), // TODO: use a normal group here.
        key: String::new(),
    });

    Proxy {
        scope: Scope::test(context.addr(), meta),
        context,
        non_exhaustive: false,
        subject_addr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use elfo_core as elfo;
    use elfo_core::{assert_msg_eq, config::AnyConfig};
    use elfo_macros::msg_raw as msg;

    #[message(elfo = elfo_core)]
    #[derive(PartialEq)]
    struct SomeMessage;

    #[message(elfo = elfo_core, ret = u32)]
    #[derive(PartialEq)]
    struct SomeRequest;

    #[message(elfo = elfo_core)]
    #[derive(PartialEq)]
    struct SomeMessage2;

    #[tokio::test]
    async fn it_handles_race_at_startup() {
        let mut proxy = super::proxy(
            ActorGroup::new().exec(|ctx| async move {
                ctx.send(SomeMessage).await.unwrap();
            }),
            AnyConfig::default(),
        )
        .await;

        assert_msg_eq!(proxy.recv().await, SomeMessage);
    }

    async fn sample() -> Proxy {
        super::proxy(
            ActorGroup::new().exec(|mut ctx| async move {
                while let Some(envelope) = ctx.recv().await {
                    let addr = envelope.sender();
                    msg!(match envelope {
                        SomeMessage => ctx.send_to(addr, SomeMessage2).await.unwrap(),
                        (SomeRequest, token) => ctx.respond(token, 42),
                    });
                }
            }),
            AnyConfig::default(),
        )
        .await
    }

    #[tokio::test]
    async fn main_proxy_works() {
        let mut proxy = sample().await;
        assert_eq!(proxy.request(SomeRequest).await, 42);
        proxy.send(SomeMessage).await;
        assert_msg_eq!(proxy.recv().await, SomeMessage2);
    }

    #[tokio::test]
    async fn subproxy_works() {
        let proxy = sample().await;
        let mut subproxy = proxy.subproxy().await;
        assert_eq!(subproxy.request(SomeRequest).await, 42);
        subproxy.send(SomeMessage).await;
        assert_msg_eq!(subproxy.recv().await, SomeMessage2);
    }
}
