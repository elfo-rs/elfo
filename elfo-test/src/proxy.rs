use std::{
    collections::BTreeMap,
    future::{self, Future},
    panic::Location,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, LazyLock,
    },
    thread,
    time::Duration,
};

use futures_intrusive::timer::{LocalTimer, StdClock, TimerService};
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;
use tokio::{sync::oneshot, task};

use elfo_core::{
    ActorGroup, Addr, Blueprint, Context, Envelope, Local, Message, MoveOwnership, Request,
    ResponseToken,
    _priv::do_start,
    addr::NodeLaunchId,
    errors::{RequestError, TrySendError},
    message, msg,
    routers::{MapRouter, Outcome},
    scope::{self, Scope},
    topology::Topology,
};

const SYNC_YIELD_COUNT: usize = 32;

/// A proxy for testing actors.
pub struct Proxy {
    context: ProxyContext,
    scope: Scope,
    subject_addr: Addr,
    recv_timeout: Duration,
}

type ProxyContext = Context<(), usize>;

impl Proxy {
    /// Returns an address of the proxy.
    pub fn addr(&self) -> Addr {
        self.context.addr()
    }

    /// Returns a launch ID of the topology.
    ///
    /// It can be used to distinguish produced artifacts (logs, dumps, metrics)
    /// from different concurrent tests if the custom implementation is used.
    pub fn node_launch_id(&self) -> NodeLaunchId {
        self.scope.node_launch_id()
    }

    /// See [`Context::send()`] for details.
    #[track_caller]
    pub fn send<M: Message>(&self, message: M) -> impl Future<Output = ()> + '_ {
        let location = Location::caller();
        self.scope.clone().within(async move {
            let name = message.name();
            if let Err(err) = self.context.send(message).await {
                panic!("cannot send {name} ({err}) at {location}");
            }
        })
    }

    /// See [`Context::send_to()`] for details.
    #[track_caller]
    pub fn send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> impl Future<Output = ()> + '_ {
        let location = Location::caller();
        self.scope.clone().within(async move {
            let name = message.name();
            if let Err(err) = self.context.send_to(recipient, message).await {
                panic!("cannot send {name} ({err}) at {location}");
            }
        })
    }

    /// See [`Context::try_send()`] for details.
    #[track_caller]
    pub fn try_send<M: Message>(&self, message: M) -> Result<(), TrySendError<M>> {
        self.scope
            .clone()
            .sync_within(|| self.context.try_send(message))
    }

    /// See [`Context::try_send_to()`] for details.
    #[track_caller]
    pub fn try_send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), TrySendError<M>> {
        self.scope
            .clone()
            .sync_within(|| self.context.try_send_to(recipient, message))
    }

    /// Same as [`Self::request`], but doesn't unwraps the error.
    pub fn request_fallible<R: Request>(
        &self,
        request: R,
    ) -> impl Future<Output = Result<R::Response, RequestError>> {
        let context = self.context.pruned();
        self.scope
            .clone()
            .within(async move { context.request(request).resolve().await })
    }

    /// See [`Context::request()`] for details.
    #[track_caller]
    pub fn request<R: Request>(&self, request: R) -> impl Future<Output = R::Response> {
        let location = Location::caller();
        let context = self.context.pruned();
        self.scope.clone().within(async move {
            let name = request.name();
            match context.request(request).resolve().await {
                Ok(response) => response,
                Err(err) => panic!("cannot send {name} ({err}) at {location}"),
            }
        })
    }

    /// Same as [`Self::request_to`], but doesn't unwraps the errors.
    pub fn request_to_fallible<R: Request>(
        &self,
        recipient: Addr,
        request: R,
    ) -> impl Future<Output = Result<R::Response, RequestError>> {
        let context = self.context.pruned();
        self.scope
            .clone()
            .within(async move { context.request_to(recipient, request).resolve().await })
    }

    /// See [`Context::request_to()`] for details.
    #[track_caller]
    pub fn request_to<R: Request>(
        &self,
        recipient: Addr,
        request: R,
    ) -> impl Future<Output = R::Response> {
        let location = Location::caller();
        let context = self.context.pruned();
        self.scope.clone().within(async move {
            let name = request.name();
            match context.request_to(recipient, request).resolve().await {
                Ok(response) => response,
                Err(err) => panic!("cannot send {name} ({err}) at {location}"),
            }
        })
    }

    /// See [`Context::respond()`] for details.
    pub fn respond<R: Request>(&self, token: ResponseToken<R>, response: R::Response) {
        self.scope
            .clone()
            .sync_within(|| self.context.respond(token, response))
    }

    /// See [`Context::recv()`] for details.
    #[track_caller]
    pub fn recv(&mut self) -> impl Future<Output = Envelope> + '_ {
        // We use a separate timer here to avoid interaction with the tokio's timer.
        static STD_CLOCK: LazyLock<StdClock> = LazyLock::new(StdClock::new);
        static TIMER_SERVICE: LazyLock<Arc<TimerService>> = LazyLock::new(|| {
            let timer_service = Arc::new(TimerService::new(&*STD_CLOCK));
            thread::spawn({
                let timer_service = timer_service.clone();
                move || loop {
                    std::thread::sleep(Duration::from_millis(25));
                    timer_service.check_expirations();
                }
            });
            timer_service
        });

        let location = Location::caller();
        self.scope.clone().within(async move {
            tokio::select! {
                Some(envelope) = self.context.recv() => {
                    envelope
                },
                _ = TIMER_SERVICE.delay(self.recv_timeout) => {
                    panic!(
                        "timeout ({:?}) while receiving a message at {}",
                        self.recv_timeout, location,
                    );
                }
            }
        })
    }

    /// See [`Context::try_recv()`] for details.
    pub async fn try_recv(&mut self) -> Option<Envelope> {
        self.scope
            .clone()
            .within(async move { self.context.try_recv().await.ok() })
            .await
    }

    /// Waits until the testable actor handles all previously sent messages.
    ///
    /// Now it's implemented as multiple calls `yield_now()`,
    /// but the implementation can be changed in the future.
    pub async fn sync(&mut self) {
        // TODO: it should probably be `request(Ping).await`.
        for _ in 0..SYNC_YIELD_COUNT {
            task::yield_now().await;
        }
    }

    /// Sets message wait time for `recv` call.
    pub fn set_recv_timeout(&mut self, recv_timeout: Duration) {
        self.recv_timeout = recv_timeout;
    }

    /// Creates a subproxy with a different address.
    /// The main purpose is to test `send_to(..)` and `request_to(..)` calls.
    pub async fn subproxy(&self) -> Proxy {
        let f = async {
            self.context
                .request_to(self.context.group(), CreateSubproxy)
                .resolve()
                .await
                .expect("cannot create a new subpoxy")
        };

        let ProxyCreated { context, scope } = self.scope.clone().within(f).await;

        Proxy {
            context: context.into_inner(),
            scope: scope.into_inner(),
            subject_addr: self.subject_addr,
            recv_timeout: self.recv_timeout,
        }
    }

    /// Waits until the testable actor finishes.
    pub async fn finished(&self) {
        let fut = self.context.finished(self.subject_addr);
        self.scope.clone().within(fut).await
    }

    /// Closes a mailbox of the proxy.
    pub fn close(&self) {
        self.scope.clone().sync_within(|| self.context.close());
    }
}

#[message(ret = ProxyCreated)]
struct CreateSubproxy;

#[message(part)]
struct ProxyCreated {
    context: Local<ProxyContext>,
    scope: Local<Scope>,
}

fn testers(tx: oneshot::Sender<ProxyCreated>) -> Blueprint {
    let tx = MoveOwnership::from(tx);
    let key = AtomicUsize::new(1); // 0 is reserved for the main proxy

    ActorGroup::new()
        .router(MapRouter::new(move |envelope| {
            msg!(match envelope {
                CreateSubproxy => Outcome::Unicast(key.fetch_add(1, Ordering::SeqCst)),
                _ => Outcome::Unicast(0),
            })
        }))
        .exec(move |mut ctx| {
            let tx = tx.clone();
            async move {
                // It would be nice to use the code in the `else` branch also for the main
                // proxy. Unfortunately, the main proxy can receive messages from the subject
                // before receiving the `CreateSubproxy` message. That's why we need to use
                // a dedicated oneshot channel for the main proxy.
                // See the `it_handles_race_at_startup` test for an example.
                if let Some(tx) = tx.take() {
                    let _ = tx.send(ProxyCreated {
                        context: ctx.into(),
                        scope: scope::expose().into(),
                    });
                } else {
                    let envelope = ctx.recv().await.unwrap();
                    let (_, token) = crate::extract_request::<CreateSubproxy>(envelope);

                    ctx.pruned().respond(
                        token,
                        ProxyCreated {
                            scope: scope::expose().into(),
                            context: ctx.into(),
                        },
                    );
                }

                // We don't track the lifetime of sent context for now, so keep the actor alive.
                future::pending::<()>().await;
            }
        })
}

#[doc(hidden)]
#[instability::unstable]
pub async fn proxy_with_route<F>(
    blueprint: Blueprint,
    route_filter: F,
    config: impl for<'de> Deserializer<'de>,
) -> Proxy
where
    F: Fn(&Envelope) -> bool + Send + Sync + 'static,
{
    // Initialize logging but skip errors if the logger is already initialized.
    // It occurs when tests are run in the same process.
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

    let subject_addr = subject.addr();

    testers.route_all_to(&subject);
    subject.route_to(&testers, route_filter);

    configurers.mount(elfo_configurer::fixture(&topology, config));
    subject.mount(blueprint);

    let (tx, rx) = oneshot::channel();
    testers.mount(self::testers(tx));
    do_start(topology, false, |_, _| future::ready(()))
        .await
        .expect("cannot start");

    let ProxyCreated { context, scope } = rx.await.expect("cannot create main proxy");

    Proxy {
        context: context.into_inner(),
        scope: scope.into_inner(),
        subject_addr,
        recv_timeout: Duration::from_millis(150),
    }
}

/// Creates a proxy for testing actors.
/// See examples in the repository for more details how to use it.
pub async fn proxy(blueprint: Blueprint, config: impl for<'de> Deserializer<'de>) -> Proxy {
    proxy_with_route(blueprint, |_| true, config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    use elfo_core::{assert_msg_eq, config::AnyConfig, message, msg};

    #[message]
    #[derive(PartialEq)]
    struct SomeMessage;

    #[message(ret = u32)]
    #[derive(PartialEq)]
    struct SomeRequest;

    #[message]
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
