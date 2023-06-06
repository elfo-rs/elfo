use std::{future::poll_fn, marker::PhantomData, pin::Pin, sync::Arc, task::Poll};

use futures::{pin_mut, Stream};
use once_cell::sync::Lazy;
use tracing::{info, trace};

use elfo_utils::unlikely;

use crate::{
    actor::{Actor, ActorStatus},
    address_book::{Addr, AddressBook},
    config::AnyConfig,
    demux::Demux,
    dumping::{Direction, Dump, Dumper, INTERNAL_CLASS},
    envelope::{Envelope, MessageKind},
    errors::{RequestError, SendError, TryRecvError, TrySendError},
    mailbox::RecvResult,
    message::{Message, Request},
    messages, msg,
    object::ObjectArc,
    request_table::ResponseToken,
    routers::Singleton,
    scope,
    source::{SourceHandle, Sources, UnattachedSource},
};

use self::{budget::Budget, stats::Stats};

mod budget;
mod stats;

static DUMPER: Lazy<Dumper> = Lazy::new(|| Dumper::new(INTERNAL_CLASS));

/// An actor execution context.
pub struct Context<C = (), K = Singleton> {
    book: AddressBook,
    actor: Option<ObjectArc>, // `None` for group's and pruned context.
    actor_addr: Addr,
    group_addr: Addr,
    demux: Demux,
    config: Arc<C>,
    key: K,
    sources: Sources,
    stage: Stage,
    stats: Stats,
    budget: Budget,
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    PreRecv,
    Working,
    Closed,
}

assert_impl_all!(Context: Send);
// TODO: !Sync?

impl<C, K> Context<C, K> {
    /// Returns the actor's address.
    #[inline]
    pub fn addr(&self) -> Addr {
        self.actor_addr
    }

    /// Returns the current group's address.
    #[inline]
    pub fn group(&self) -> Addr {
        self.group_addr
    }

    /// Returns the actual config.
    #[inline]
    pub fn config(&self) -> &C {
        &self.config
    }

    /// Returns the actor's key.
    #[inline]
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Attaches the provided source to the context.
    pub fn attach<S1: SourceHandle>(&mut self, source: UnattachedSource<S1>) -> S1 {
        source.attach_to(&mut self.sources)
    }

    /// Updates the actor's status.
    ///
    /// ```ignore
    /// ctx.set_status(ActorStatus::ALARMING.with_details("something wrong"));
    /// ```
    pub fn set_status(&self, status: ActorStatus) {
        ward!(self.actor.as_ref().and_then(|o| o.as_actor())).set_status(status);
    }

    /// Closes the mailbox, that leads to returning `None` from `recv()` and
    /// `try_recv()` after handling all available messages in the mailbox.
    ///
    /// Returns `true` if the mailbox has just been closed.
    pub fn close(&self) -> bool {
        ward!(self.actor.as_ref().and_then(|o| o.as_actor()), return false).close()
    }

    /// Sends a message using the routing system.
    ///
    /// Returns `Err` if the message hasn't reached any mailboxes.
    ///
    /// # Example
    /// ```ignore
    /// // Fire and forget.
    /// let _ = ctx.send(SomethingHappened).await;
    ///
    /// // Fire or fail.
    /// ctx.send(SomethingHappened).await?;
    ///
    /// // Fire or log.
    /// if let Ok(err) = ctx.send(SomethingHappened).await {
    ///     warn!("...", error = err);
    /// }
    /// ```
    pub async fn send<M: Message>(&self, message: M) -> Result<(), SendError<M>> {
        let kind = MessageKind::Regular {
            sender: self.actor_addr,
        };
        self.do_send(message, kind).await
    }

    /// Tries to send a message using the routing system.
    ///
    /// Returns
    /// * `Ok(())` if the message has been added to any mailbox.
    /// * `Err(Full(_))` if some mailboxes are full.
    /// * `Err(Closed(_))` otherwise.
    ///
    /// # Example
    /// ```ignore
    /// // Fire and forget.
    /// let _ = ctx.try_send(SomethingHappened);
    ///
    /// // Fire or fail.
    /// ctx.try_send(SomethingHappened)?;
    ///
    /// // Fire or log.
    /// if let Err(err) = ctx.try_send(SomethingHappened) {
    ///     warn!("...", error = err);
    /// }
    /// ```
    pub fn try_send<M: Message>(&self, message: M) -> Result<(), TrySendError<M>> {
        let kind = MessageKind::Regular {
            sender: self.actor_addr,
        };

        self.stats.on_sent_message(&message);

        trace!("> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m(&message) {
            permit.record(Dump::message(&message, &kind, Direction::Out));
        }

        let envelope = Envelope::new(message, kind).upcast();
        let addrs = self.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(TrySendError::Closed(e2m(envelope)));
        }

        if addrs.len() == 1 {
            return match self.book.get(addrs[0]) {
                Some(object) => object
                    .try_send(Addr::NULL, envelope)
                    .map_err(|err| err.map(e2m)),
                None => Err(TrySendError::Closed(e2m(envelope))),
            };
        }

        let mut unused = None;
        let mut has_full = false;
        let mut success = false;

        // TODO: use the visitor pattern in order to avoid extra cloning.
        for addr in addrs {
            let envelope = unused.take().or_else(|| envelope.duplicate(&self.book));
            let envelope = ward!(envelope, break);

            match self.book.get(addr) {
                Some(object) => match object.try_send(Addr::NULL, envelope) {
                    Ok(()) => success = true,
                    Err(err) => {
                        has_full |= err.is_full();
                        unused = Some(err.into_inner());
                    }
                },
                None => unused = Some(envelope),
            };
        }

        if success {
            Ok(())
        } else if has_full {
            Err(TrySendError::Full(e2m(envelope)))
        } else {
            Err(TrySendError::Closed(e2m(envelope)))
        }
    }

    /// Returns a request builder.
    ///
    /// # Example
    /// ```ignore
    /// // Request and wait for a response.
    /// let response = ctx.request(SomeCommand).resolve().await?;
    ///
    /// // Request and wait for all responses.
    /// for result in ctx.request(SomeCommand).all().resolve().await {
    ///     // ...
    /// }
    /// ```
    #[inline]
    pub fn request<R: Request>(&self, request: R) -> RequestBuilder<'_, C, K, R, Any> {
        RequestBuilder::new(self, request)
    }

    /// Returns a request builder to the specified recipient.
    ///
    /// # Example
    /// ```ignore
    /// // Request and wait for a response.
    /// let response = ctx.request_to(addr, SomeCommand).resolve().await?;
    ///
    /// // Request and wait for all responses.
    /// for result in ctx.request_to(addr, SomeCommand).all().resolve().await {
    ///     // ...
    /// }
    /// ```
    #[inline]
    pub fn request_to<R: Request>(
        &self,
        recipient: Addr,
        request: R,
    ) -> RequestBuilder<'_, C, K, R, Any> {
        RequestBuilder::new(self, request).to(recipient)
    }

    async fn do_send<M: Message>(&self, message: M, kind: MessageKind) -> Result<(), SendError<M>> {
        self.stats.on_sent_message(&message);

        trace!("> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m(&message) {
            permit.record(Dump::message(&message, &kind, Direction::Out));
        }

        let envelope = Envelope::new(message, kind).upcast();
        let addrs = self.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(SendError(e2m(envelope)));
        }

        if addrs.len() == 1 {
            let recipient = addrs[0];
            return match self.book.get_owned(recipient) {
                Some(object) => object
                    .send(self, recipient, envelope)
                    .await
                    .map_err(|err| SendError(e2m(err.0))),
                None => Err(SendError(e2m(envelope))),
            };
        }

        let mut unused = None;
        let mut success = false;

        // TODO: use the visitor pattern in order to avoid extra cloning.
        // TODO: send concurrently.
        for addr in addrs {
            let envelope = unused.take().or_else(|| envelope.duplicate(&self.book));
            let envelope = ward!(envelope, break);

            match self.book.get_owned(addr) {
                Some(object) => {
                    unused = object
                        .send(self, addr, envelope)
                        .await
                        .err()
                        .map(|err| err.0);
                    if unused.is_none() {
                        success = true;
                    }
                }
                None => unused = Some(envelope),
            };
        }

        if success {
            Ok(())
        } else {
            Err(SendError(e2m(envelope)))
        }
    }

    /// Sends a message to the specified recipient.
    ///
    /// Returns `Err` if the message hasn't reached any mailboxes.
    ///
    /// # Example
    /// ```ignore
    /// // Fire and forget.
    /// let _ = ctx.send_to(addr, SomethingHappened).await;
    ///
    /// // Fire or fail.
    /// ctx.send_to(addr, SomethingHappened).await?;
    ///
    /// // Fire or log.
    /// if let Some(err) = ctx.send_to(addr, SomethingHappened).await {
    ///     warn!("...", error = err);
    /// }
    /// ```
    pub async fn send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), SendError<M>> {
        let kind = MessageKind::Regular {
            sender: self.actor_addr,
        };
        self.do_send_to(recipient, message, kind).await
    }

    #[doc(hidden)]
    #[stability::unstable]
    pub async fn do_send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
        kind: MessageKind,
    ) -> Result<(), SendError<M>> {
        self.stats.on_sent_message(&message);

        trace!(to = %recipient, "> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m(&message) {
            permit.record(Dump::message(&message, &kind, Direction::Out));
        }

        let entry = self.book.get_owned(recipient);
        let object = ward!(entry, return Err(SendError(message)));
        let envelope = Envelope::new(message, kind);
        let fut = object.send(self, recipient, envelope.upcast());
        let result = fut.await;
        result.map_err(|err| SendError(e2m(err.0)))
    }

    /// Tries to send a message to the specified recipient.
    ///
    /// Returns `Err` if the message hasn't reached mailboxes or they are full.
    ///
    /// # Example
    /// ```ignore
    /// // Fire and forget.
    /// let _ = ctx.try_send_to(addr, SomethingHappened);
    ///
    /// // Fire or fail.
    /// ctx.try_send_to(addr, SomethingHappened)?;
    ///
    /// // Fire or log.
    /// if let Some(err) = ctx.try_send_to(addr, SomethingHappened) {
    ///     warn!("...", error = err);
    /// }
    /// ```
    pub fn try_send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), TrySendError<M>> {
        self.stats.on_sent_message(&message);

        let kind = MessageKind::Regular {
            sender: self.actor_addr,
        };

        trace!(to = %recipient, "> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m(&message) {
            permit.record(Dump::message(&message, &kind, Direction::Out));
        }

        let entry = self.book.get(recipient);
        let object = ward!(entry, return Err(TrySendError::Closed(message)));
        let envelope = Envelope::new(message, kind);

        object
            .try_send(recipient, envelope.upcast())
            .map_err(|err| err.map(e2m))
    }

    /// Responds to the requester with the provided response.
    ///
    /// The token can be used only once.
    ///
    /// ```ignore
    /// msg!(match envelope {
    ///     (SomeRequest, token) => {
    ///         ctx.respond(token, SomeResponse);
    ///     }
    /// })
    /// ```
    pub fn respond<R: Request>(&self, token: ResponseToken<R>, message: R::Response) {
        if token.is_forgotten() {
            return;
        }

        let sender = token.sender;
        let message = R::Wrapper::from(message);
        self.stats.on_sent_message(&message);

        let kind = MessageKind::Response {
            sender: self.addr(),
            request_id: token.request_id,
        };

        trace!(to = %sender, "> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m(&message) {
            permit.record(Dump::message(&message, &kind, Direction::Out));
        }

        let envelope = Envelope::new(message, kind).upcast();
        let object = ward!(self.book.get(token.sender));
        let actor = ward!(object.as_actor());
        actor
            .request_table()
            .respond(token.into_untyped(), envelope);
    }

    /// Receives the next envelope from the mailbox or sources.
    /// If the envelope isn't available, the method waits for the next one.
    /// If the mailbox is closed, `None` is returned.
    ///
    /// # Budget
    ///
    /// The method returns the execution back to the runtime once the actor's
    /// budget has been exhausted. It prevents the actor from blocking the
    /// runtime for too long.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel safe. However, using `select!` requires handling
    /// tracing on your own, so avoid it if possible (use sources instead).
    ///
    /// # Panics
    ///
    /// If the method is called again after `None` is returned.
    ///
    /// # Example
    ///
    /// ```
    /// # use elfo_core as elfo;
    /// # async fn exec(mut ctx: elfo::Context) {
    /// # use elfo::{message, msg};
    /// # #[message]
    /// # struct SomethingHappened;
    /// while let Some(envelope) = ctx.recv().await {
    ///     msg!(match envelope {
    ///         SomethingHappened => { /* ... */ },
    ///     });
    /// }
    /// # }
    pub async fn recv(&mut self) -> Option<Envelope>
    where
        C: 'static,
    {
        'outer: loop {
            // TODO: reset if the mailbox is empty.
            self.budget.acquire().await;

            self.pre_recv();

            let envelope = 'received: {
                let mailbox_fut = self.actor.as_ref()?.as_actor()?.recv();
                pin_mut!(mailbox_fut);

                tokio::select! {
                    result = mailbox_fut => match result {
                        RecvResult::Data(envelope) => {
                            break 'received envelope;
                        },
                        RecvResult::Closed(trace_id) => {
                            scope::set_trace_id(trace_id);
                            let actor = self.actor.as_ref()?.as_actor()?;
                            on_input_closed(&mut self.stage, actor);
                            return None;
                        }
                    },
                    option = self.sources.next(), if !self.sources.is_empty() => {
                        let envelope = ward!(option, continue 'outer);
                        break 'received envelope;
                    },
                }
            };

            if let Some(envelope) = self.post_recv(envelope) {
                return Some(envelope);
            }
        }
    }

    /// Receives the next envelope from the mailbox or sources without waiting.
    /// If the envelope isn't available, `Err(TryRecvError::Empty)` is returned.
    /// If the mailbox is closed, `Err(TryRecvError::Closed)` is returned.
    /// Useful to batch message processing.
    ///
    /// The method is async due to the following reasons:
    /// 1. To poll sources, not only the mailbox.
    /// 2. To respect the actor budget (see below).
    ///
    /// # Budget
    ///
    /// The method returns the execution back to the runtime once the actor's
    /// budget has been exhausted. It prevents the actor from blocking the
    /// runtime for too long.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel safe. However, using `select!` requires handling
    /// tracing on your own, so avoid it if possible (use sources instead).
    ///
    /// # Panics
    ///
    /// If the method is called again after `Err(TryRecvError::Closed)`.
    ///
    /// # Example
    ///
    /// Handle all available messages:
    /// ```
    /// # use elfo_core as elfo;
    /// # async fn exec(mut ctx: elfo::Context) {
    /// # fn handle_batch(_batch: impl Iterator<Item = elfo::Envelope>) {}
    /// let mut batch = Vec::new();
    ///
    /// loop {
    ///     match ctx.try_recv().await {
    ///         Ok(envelope) => batch.push(envelope),
    ///         Err(err) => {
    ///             handle_batch(batch.drain(..));
    ///
    ///             if err.is_empty() {
    ///                 // Wait for the next batch.
    ///                 if let Some(envelope) = ctx.recv().await {
    ///                     batch.push(envelope);
    ///                     continue;
    ///                 }
    ///             }
    ///
    ///             break;
    ///         },
    ///     }
    /// }
    /// # }
    /// ```
    pub async fn try_recv(&mut self) -> Result<Envelope, TryRecvError>
    where
        C: 'static,
    {
        #[allow(clippy::never_loop)] // false positive
        loop {
            self.budget.acquire().await;

            self.pre_recv();

            let envelope = 'received: {
                let actor = ward!(
                    self.actor.as_ref().and_then(|o| o.as_actor()),
                    return Err(TryRecvError::Closed)
                );

                // TODO: poll mailbox and sources fairly.
                match actor.try_recv() {
                    Some(RecvResult::Data(envelope)) => {
                        break 'received envelope;
                    }
                    Some(RecvResult::Closed(trace_id)) => {
                        scope::set_trace_id(trace_id);
                        on_input_closed(&mut self.stage, actor);
                        return Err(TryRecvError::Closed);
                    }
                    None => {}
                }

                if !self.sources.is_empty() {
                    let envelope = poll_fn(|cx| match Pin::new(&mut self.sources).poll_next(cx) {
                        Poll::Ready(Some(envelope)) => Poll::Ready(Some(envelope)),
                        _ => Poll::Ready(None),
                    })
                    .await;

                    if let Some(envelope) = envelope {
                        break 'received envelope;
                    }
                }

                self.stats.on_empty_mailbox();
                return Err(TryRecvError::Empty);
            };

            if let Some(envelope) = self.post_recv(envelope) {
                return Ok(envelope);
            }
        }
    }

    fn pre_recv(&mut self) {
        self.stats.on_recv();

        if unlikely(self.stage == Stage::Closed) {
            panic!("calling `recv()` or `try_recv()` after `None` is returned, an infinite loop?");
        }

        if unlikely(self.stage == Stage::PreRecv) {
            let actor = ward!(self.actor.as_ref().and_then(|o| o.as_actor()));
            if actor.is_initializing() {
                actor.set_status(ActorStatus::NORMAL);
            }
            self.stage = Stage::Working;
        }
    }

    fn post_recv(&mut self, envelope: Envelope) -> Option<Envelope>
    where
        C: 'static,
    {
        self.budget.decrement();

        scope::set_trace_id(envelope.trace_id());

        let envelope = msg!(match envelope {
            (messages::UpdateConfig { config }, token) => {
                self.config = config.get_user::<C>().clone();
                info!("config updated");
                let message = messages::ConfigUpdated {};
                let kind = MessageKind::Regular {
                    sender: self.actor_addr,
                };
                let envelope = Envelope::new(message, kind).upcast();
                self.respond(token, Ok(()));
                envelope
            }
            envelope => envelope,
        });

        let message = envelope.message();
        trace!("< {:?}", message);
        if let Some(permit) = DUMPER.acquire_m(message) {
            let kind = envelope.message_kind();
            permit.record(Dump::message(message, kind, Direction::In));
        }

        // We should change the status after dumping the original message
        // in order to see `ActorStatusReport` after that message.
        if envelope.is::<messages::Terminate>() {
            self.set_status(ActorStatus::TERMINATING);
        }

        self.stats.on_received_envelope(&envelope);

        msg!(match envelope {
            (messages::Ping, token) => {
                self.respond(token, ());
                None
            }
            envelope => Some(envelope),
        })
    }

    /// This is a part of private API for now.
    /// We should provide a way to handle it asynchronous.
    #[doc(hidden)]
    pub async fn finished(&self, addr: Addr) {
        ward!(self.book.get_owned(addr)).finished().await;
    }

    /// Used to get the typed config from `ValidateConfig`.
    /// ```ignore
    /// msg!(match envelope {
    ///     (ValidateConfig { config, .. }, token) => {
    ///         let new_config = ctx.unpack_config(&config);
    ///         ctx.respond(token, Err("oops".into()));
    ///     }
    /// })
    /// ```
    pub fn unpack_config<'c>(&self, config: &'c AnyConfig) -> &'c C
    where
        C: for<'de> serde::Deserialize<'de> + 'static,
    {
        config.get_user()
    }

    /// Produces a new context that can be used for sending messages only.
    ///
    /// Pruned contexts are likely to be removed in favor of `Output`.
    pub fn pruned(&self) -> Context {
        Context {
            book: self.book.clone(),
            actor: None,
            actor_addr: self.actor_addr,
            group_addr: self.group_addr,
            demux: self.demux.clone(),
            config: Arc::new(()),
            key: Singleton,
            sources: Sources::new(),
            stage: self.stage,
            stats: Stats::empty(),
            budget: self.budget.clone(),
        }
    }

    pub(crate) fn book(&self) -> &AddressBook {
        &self.book
    }

    pub(crate) fn with_config<C1>(self, config: Arc<C1>) -> Context<C1, K> {
        Context {
            book: self.book,
            actor: self.actor,
            actor_addr: self.actor_addr,
            group_addr: self.group_addr,
            demux: self.demux,
            config,
            key: self.key,
            sources: self.sources,
            stage: self.stage,
            stats: self.stats,
            budget: self.budget,
        }
    }

    pub(crate) fn with_addr(mut self, addr: Addr) -> Self {
        self.actor = self.book.get_owned(addr);
        assert!(self.actor.is_some());
        self.actor_addr = addr;
        self.stats = Stats::startup();
        self
    }

    pub(crate) fn with_group(mut self, group: Addr) -> Self {
        self.group_addr = group;
        self
    }

    pub(crate) fn with_key<K1>(self, key: K1) -> Context<C, K1> {
        Context {
            book: self.book,
            actor: self.actor,
            actor_addr: self.actor_addr,
            group_addr: self.group_addr,
            demux: self.demux,
            config: self.config,
            key,
            sources: self.sources,
            stage: self.stage,
            stats: self.stats,
            budget: self.budget,
        }
    }
}

fn e2m<M: Message>(envelope: Envelope) -> M {
    envelope.do_downcast::<M>().into_message()
}

#[cold]
fn on_input_closed(stage: &mut Stage, actor: &Actor) {
    if !actor.is_terminating() {
        actor.set_status(ActorStatus::TERMINATING);
    }
    *stage = Stage::Closed;
    trace!("input closed");
}

impl Context {
    pub(crate) fn new(book: AddressBook, demux: Demux) -> Self {
        Self {
            book,
            actor: None,
            actor_addr: Addr::NULL,
            group_addr: Addr::NULL,
            demux,
            config: Arc::new(()),
            key: Singleton,
            sources: Sources::new(),
            stage: Stage::PreRecv,
            stats: Stats::empty(),
            budget: Budget::default(),
        }
    }
}

// TODO(v0.2): remove this instance.
impl<C, K: Clone> Clone for Context<C, K> {
    fn clone(&self) -> Self {
        Self {
            book: self.book.clone(),
            actor: self.book.get_owned(self.actor_addr),
            actor_addr: self.actor_addr,
            group_addr: self.group_addr,
            demux: self.demux.clone(),
            config: self.config.clone(),
            key: self.key.clone(),
            sources: Sources::new(),
            stage: self.stage,
            stats: Stats::empty(),
            budget: self.budget.clone(),
        }
    }
}

#[must_use]
pub struct RequestBuilder<'c, C, K, R, M> {
    context: &'c Context<C, K>,
    request: R,
    to: Option<Addr>,
    marker: PhantomData<M>,
}

pub struct Any;
pub struct All;

impl<'c, C, K, R> RequestBuilder<'c, C, K, R, Any> {
    fn new(context: &'c Context<C, K>, request: R) -> Self {
        Self {
            context,
            request,
            to: None,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn all(self) -> RequestBuilder<'c, C, K, R, All> {
        RequestBuilder {
            context: self.context,
            request: self.request,
            to: self.to,
            marker: PhantomData,
        }
    }
}

impl<'c, C, K, R, M> RequestBuilder<'c, C, K, R, M> {
    /// Specified the recipient of the request.
    #[inline]
    fn to(mut self, addr: Addr) -> Self {
        self.to = Some(addr);
        self
    }
}

// TODO: add `pub async fn id() { ... }`
impl<'c, C: 'static, K, R: Request> RequestBuilder<'c, C, K, R, Any> {
    /// Waits for the response.
    pub async fn resolve(self) -> Result<R::Response, RequestError<R>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.actor_addr;
        let object = self.context.book.get_owned(this).expect("invalid addr");
        let actor = object.as_actor().expect("can be called only on actors");
        let token = actor
            .request_table()
            .new_request(self.context.book.clone(), false);
        let request_id = token.request_id;
        let kind = MessageKind::RequestAny(token);

        let res = if let Some(recipient) = self.to {
            self.context.do_send_to(recipient, self.request, kind).await
        } else {
            self.context.do_send(self.request, kind).await
        };

        if let Err(err) = res {
            return Err(RequestError::Closed(err.0));
        }

        let mut data = actor.request_table().wait(request_id).await;
        if let Some(Some(envelope)) = data.pop() {
            let envelope = envelope.do_downcast::<R::Wrapper>();

            // TODO: increase a counter.
            let message = envelope.message();
            trace!("< {:?}", message);
            if let Some(permit) = DUMPER.acquire_m(message) {
                let kind = envelope.message_kind();
                permit.record(Dump::message(message, kind, Direction::In));
            }
            Ok(envelope.into_message().into())
        } else {
            // TODO: should we dump it and increase a counter?
            Err(RequestError::Ignored)
        }
    }
}

impl<'c, C: 'static, K, R: Request> RequestBuilder<'c, C, K, R, All> {
    /// Waits for the responses.
    pub async fn resolve(self) -> Vec<Result<R::Response, RequestError<R>>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.actor_addr;
        let object = self.context.book.get_owned(this).expect("invalid addr");
        let actor = object.as_actor().expect("can be called only on actors");
        let token = actor
            .request_table()
            .new_request(self.context.book.clone(), true);
        let request_id = token.request_id;
        let kind = MessageKind::RequestAll(token);

        let res = if let Some(recipient) = self.to {
            self.context.do_send_to(recipient, self.request, kind).await
        } else {
            self.context.do_send(self.request, kind).await
        };

        if let Err(err) = res {
            return vec![Err(RequestError::Closed(err.0))];
        }

        actor
            .request_table()
            .wait(request_id)
            .await
            .into_iter()
            .map(|opt| match opt {
                Some(envelope) => Ok(envelope.do_downcast::<R::Wrapper>()),
                None => Err(RequestError::Ignored),
            })
            .map(|res| {
                let envelope = res?;

                // TODO: increase a counter.
                let message = envelope.message();
                trace!("< {:?}", message);

                // TODO: `acquire_many` or even unconditionally?
                if let Some(permit) = DUMPER.acquire_m(message) {
                    let kind = envelope.message_kind();
                    permit.record(Dump::message(message, kind, Direction::In));
                }
                Ok(envelope.into_message().into())
            })
            .collect()
    }
}
