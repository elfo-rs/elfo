use std::{collections::BTreeMap, sync::Arc};

use futures_intrusive::channel::shared;
use serde::{de::Deserializer, Deserialize};
use serde_value::Value;

use elfo_core::{
    ActorGroup, Context, Envelope, Message, Request, ResponseToken, Schema, Topology,
    _priv::do_start,
};

pub struct Proxy {
    context: Context,
}

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
        self.context.recv().await.expect("closed mailbox")
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
    configurers.mount(elfo_core::configurers::fixture(&topology, config));
    subject.mount(schema);

    let (tx, rx) = shared::oneshot_channel();
    testers.mount(self::testers(tx));

    do_start(topology).await.expect("cannot start");

    let context = rx.receive().await.expect("cannot receive tester's context");
    Proxy { context }
}
