use std::{marker::PhantomData, sync::Arc};

use futures::{future::poll_fn, pin_mut};
use once_cell::sync::Lazy;
use tracing::{error, info, trace};

use crate::{self as elfo};
use elfo_macros::msg_raw as msg;

use crate::{
    actor::{Actor, ActorStatus},
    addr::Addr,
    address_book::AddressBook,
    config::AnyConfig,
    demux::Demux,
    dumping::{self, Direction, Dump, Dumper, INTERNAL_CLASS},
    envelope::{Envelope, MessageKind},
    errors::{RequestError, SendError, TryRecvError, TrySendError},
    mailbox::RecvResult,
    message::{Message, Request},
    messages,
    request_table::ResponseToken,
    routers::Singleton,
    scope,
    source::{Combined, Source},
};

use self::stats::Stats;

mod stats;

static DUMPER: Lazy<Dumper> = Lazy::new(|| Dumper::new(INTERNAL_CLASS));

/// An actor execution context.
pub struct Context<C = (), K = Singleton, S = ()> {
    book: AddressBook,
    addr: Addr,
    group: Addr,
    demux: Demux,
    config: Arc<C>,
    key: K,
    source: S,
    stage: Stage,
    stats: Stats,
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    PreRecv,
    Working,
    Closed,
}

assert_impl_all!(Context: Send);
// TODO: !Sync?

impl<C, K, S> Context<C, K, S> {
    /// Returns the actor's address.
    #[inline]
    pub fn addr(&self) -> Addr {
        self.addr
    }

    /// Returns the current group's address.
    #[inline]
    pub fn group(&self) -> Addr {
        self.group
    }

    #[deprecated]
    #[doc(hidden)]
    #[cfg(feature = "test-util")]
    pub fn set_addr(&mut self, addr: Addr) {
        self.addr = addr;
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

    /// Transforms the context to another one with the provided source.
    pub fn with<S1>(self, source: S1) -> Context<C, K, Combined<S, S1>> {
        Context {
            book: self.book,
            addr: self.addr,
            group: self.group,
            demux: self.demux,
            config: self.config,
            key: self.key,
            source: Combined::new(self.source, source),
            stage: Stage::PreRecv,
            stats: Stats::default(),
        }
    }

    /// Updates the actor's status.
    ///
    /// ```ignore
    /// ctx.set_status(ActorStatus::ALARMING.with_details("something wrong"));
    /// ```
    pub fn set_status(&self, status: ActorStatus) {
        let object = ward!(self.book.get_owned(self.addr));
        let actor = ward!(object.as_actor());
        actor.set_status(status);
    }

    /// Closes the mailbox, that leads to returning `None` from `recv()` and
    /// `try_recv()` after handling all available messages in the mailbox.
    ///
    /// Returns `true` if the mailbox has just been closed.
    pub fn close(&self) -> bool {
        let object = ward!(self.book.get_owned(self.addr), return false);
        ward!(object.as_actor(), return false).close()
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
        let kind = MessageKind::Regular { sender: self.addr };
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
        let kind = MessageKind::Regular { sender: self.addr };

        // XXX: unify with `do_send`.
        self.stats.sent_messages_total::<M>();

        trace!("> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m::<M>() {
            permit.record(Dump::message(message.clone(), &kind, Direction::Out));
        }

        let envelope = Envelope::new(message, kind).upcast();
        let addrs = self.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(TrySendError::Closed(e2m(envelope)));
        }

        if addrs.len() == 1 {
            return match self.book.get(addrs[0]) {
                Some(object) => object.try_send(envelope).map_err(|err| err.map(e2m)),
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
                Some(object) => match object.try_send(envelope) {
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
    pub fn request<R: Request>(&self, request: R) -> RequestBuilder<'_, C, K, S, R, Any> {
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
    ) -> RequestBuilder<'_, C, K, S, R, Any> {
        RequestBuilder::new(self, request).to(recipient)
    }

    async fn do_send<M: Message>(&self, message: M, kind: MessageKind) -> Result<(), SendError<M>> {
        self.stats.sent_messages_total::<M>();

        trace!("> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m::<M>() {
            permit.record(Dump::message(message.clone(), &kind, Direction::Out));
        }

        let envelope = Envelope::new(message, kind).upcast();
        let addrs = self.demux.filter(&envelope);

        if addrs.is_empty() {
            return Err(SendError(e2m(envelope)));
        }

        if addrs.len() == 1 {
            return match self.book.get_owned(addrs[0]) {
                Some(object) => object
                    .send(self, envelope)
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
                    unused = object.send(self, envelope).await.err().map(|err| err.0);
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
        let kind = MessageKind::Regular { sender: self.addr };
        self.do_send_to(recipient, message, kind).await
    }

    async fn do_send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
        kind: MessageKind,
    ) -> Result<(), SendError<M>> {
        self.stats.sent_messages_total::<M>();

        trace!(to = %recipient, "> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m::<M>() {
            permit.record(Dump::message(message.clone(), &kind, Direction::Out));
        }

        let entry = self.book.get_owned(recipient);
        let object = ward!(entry, return Err(SendError(message)));
        let envelope = Envelope::new(message, kind);
        let fut = object.send(self, envelope.upcast());
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
    /// let _ = ctx.send(SomethingHappened).await;
    ///
    /// // Fire or fail.
    /// ctx.send(SomethingHappened).await?;
    ///
    /// // Fire or log.
    /// if let Some(err) = ctx.send(SomethingHappened).await {
    ///     warn!("...", error = err);
    /// }
    /// ```
    pub fn try_send_to<M: Message>(
        &self,
        recipient: Addr,
        message: M,
    ) -> Result<(), TrySendError<M>> {
        self.stats.sent_messages_total::<M>();

        let kind = MessageKind::Regular { sender: self.addr };

        trace!(to = %recipient, "> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m::<M>() {
            permit.record(Dump::message(message.clone(), &kind, Direction::Out));
        }

        let entry = self.book.get(recipient);
        let object = ward!(entry, return Err(TrySendError::Closed(message)));
        let envelope = Envelope::new(message, kind);

        object
            .try_send(envelope.upcast())
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

        self.stats.sent_messages_total::<R::Wrapper>();

        let sender = token.sender;
        let message = R::Wrapper::from(message);
        let kind = MessageKind::Response {
            sender: self.addr(),
            request_id: token.request_id,
        };

        trace!(to = %sender, "> {:?}", message);
        if let Some(permit) = DUMPER.acquire_m::<R>() {
            permit.record(Dump::message(message.clone(), &kind, Direction::Out));
        }

        let envelope = Envelope::new(message, kind).upcast();
        let object = ward!(self.book.get(token.sender));
        let actor = ward!(object.as_actor());
        actor
            .request_table()
            .respond(token.into_untyped(), envelope);
    }

    /// Receives the next envelope.
    ///
    /// # Panics
    /// If the method is called again after returning `None`.
    ///
    /// # Examples
    /// ```ignore
    /// while let Some(envelope) = ctx.recv().await {
    ///     msg!(match envelope {
    ///         SomethingHappened => /* ... */,
    ///     })
    /// }
    /// ```
    pub async fn recv(&mut self) -> Option<Envelope>
    where
        C: 'static,
        S: Source,
    {
        loop {
            self.stats.message_handling_time_seconds();

            if self.stage == Stage::Closed {
                on_recv_after_close();
            }

            // TODO: cache `OwnedEntry`?
            let object = self.book.get_owned(self.addr)?;
            let actor = object.as_actor()?;

            if self.stage == Stage::PreRecv {
                on_first_recv(&mut self.stage, actor);
            }

            // TODO: reset `trace_id` to `None`?

            let mailbox_fut = actor.recv();
            pin_mut!(mailbox_fut);

            let source_fut = poll_fn(|cx| self.source.poll_recv(cx));
            pin_mut!(source_fut);

            tokio::select! { biased;
                result = mailbox_fut => match result {
                    RecvResult::Data(envelope) => {
                        if let Some(envelope) = self.post_recv(envelope) {
                            return Some(envelope);
                        }
                    },
                    RecvResult::Closed(trace_id) => {
                        scope::set_trace_id(trace_id);
                        on_input_closed(&mut self.stage, actor);
                        return None;
                    }
                },
                option = source_fut => {
                    // Sources cannot return `None` for now.
                    let envelope = option.expect("source cannot return None");

                    if let Some(envelope) = self.post_recv(envelope) {
                        return Some(envelope);
                    }
                },
            }
        }
    }

    /// Receives the next envelope without waiting.
    ///
    /// # Panics
    /// If the method is called again after returning
    /// `Err(TryRecvError::Closed)`.
    ///
    /// # Examples
    /// ```ignore
    /// // Iterate over all available messages.
    /// while let Ok(envelope) = ctx.try_recv() {
    ///     msg!(match envelope {
    ///         SomethingHappened => /* ... */,
    ///     })
    /// }
    /// ```
    pub fn try_recv(&mut self) -> Result<Envelope, TryRecvError>
    where
        C: 'static,
    {
        loop {
            self.stats.message_handling_time_seconds();

            if self.stage == Stage::Closed {
                on_recv_after_close();
            }

            let object = self.book.get(self.addr).ok_or(TryRecvError::Closed)?;
            let actor = object.as_actor().ok_or(TryRecvError::Closed)?;

            if self.stage == Stage::PreRecv {
                on_first_recv(&mut self.stage, actor);
            }

            // TODO: poll the sources.
            match actor.try_recv() {
                Some(RecvResult::Data(envelope)) => {
                    drop(object);

                    if let Some(envelope) = self.post_recv(envelope) {
                        return Ok(envelope);
                    }
                }
                Some(RecvResult::Closed(trace_id)) => {
                    scope::set_trace_id(trace_id);
                    on_input_closed(&mut self.stage, actor);
                    return Err(TryRecvError::Closed);
                }
                None => return Err(TryRecvError::Empty),
            }
        }
    }

    fn post_recv(&mut self, envelope: Envelope) -> Option<Envelope>
    where
        C: 'static,
    {
        scope::set_trace_id(envelope.trace_id());

        let envelope = msg!(match envelope {
            (messages::UpdateConfig { config }, token) => {
                self.config = config.get_user::<C>().clone();
                info!("config updated");
                let message = messages::ConfigUpdated {};
                let kind = MessageKind::Regular { sender: self.addr };
                let envelope = Envelope::new(message, kind).upcast();
                self.respond(token, Ok(()));
                envelope
            }
            envelope => envelope,
        });

        let message = envelope.message();
        trace!("< {:?}", message);

        if message.dumping_allowed() {
            if let Some(permit) = DUMPER.acquire() {
                // TODO: reuse `Dump::message`, it requires `AnyMessage: Message`.
                let dump = Dump::builder()
                    .direction(Direction::In)
                    .message_name(message.name())
                    .message_protocol(message.protocol())
                    .message_kind(dumping::MessageKind::from_message_kind(
                        envelope.message_kind(),
                    ))
                    .do_finish(message.erase());

                permit.record(dump);
            }
        }

        // We should change the status after dumping the original message
        // in order to see `ActorStatusReport` after that message.
        if envelope.is::<messages::Terminate>() {
            self.set_status(ActorStatus::TERMINATING);
        }

        self.stats.message_waiting_time_seconds(&envelope);

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
            addr: self.addr,
            group: self.group,
            demux: self.demux.clone(),
            config: Arc::new(()),
            key: Singleton,
            source: (),
            stage: self.stage,
            stats: Stats::default(),
        }
    }

    pub(crate) fn book(&self) -> &AddressBook {
        &self.book
    }

    pub(crate) fn with_config<C1>(self, config: Arc<C1>) -> Context<C1, K, S> {
        Context {
            book: self.book,
            addr: self.addr,
            group: self.group,
            demux: self.demux,
            config,
            key: self.key,
            source: self.source,
            stage: self.stage,
            stats: Stats::default(),
        }
    }

    pub(crate) fn with_addr(mut self, addr: Addr) -> Self {
        self.addr = addr;
        self
    }

    pub(crate) fn with_group(mut self, group: Addr) -> Self {
        self.group = group;
        self
    }

    pub(crate) fn with_key<K1>(self, key: K1) -> Context<C, K1, S> {
        Context {
            book: self.book,
            addr: self.addr,
            group: self.group,
            demux: self.demux,
            config: self.config,
            key,
            source: self.source,
            stage: self.stage,
            stats: Stats::default(),
        }
    }
}

fn e2m<M: Message>(envelope: Envelope) -> M {
    envelope.do_downcast::<M>().into_message()
}

#[cold]
fn on_first_recv(stage: &mut Stage, actor: &Actor) {
    if actor.is_initializing() {
        actor.set_status(ActorStatus::NORMAL);
    }
    *stage = Stage::Working;
}

#[cold]
fn on_input_closed(stage: &mut Stage, actor: &Actor) {
    if !actor.is_terminating() {
        actor.set_status(ActorStatus::TERMINATING);
    }
    *stage = Stage::Closed;
    trace!("input closed");
}

#[cold]
fn on_recv_after_close() {
    error!("calling `recv()` or `try_recv()` after `None` is returned, an infinite loop?");
    panic!("suicide");
}

impl Context {
    pub(crate) fn new(book: AddressBook, demux: Demux) -> Self {
        Self {
            book,
            addr: Addr::NULL,
            group: Addr::NULL,
            demux,
            config: Arc::new(()),
            key: Singleton,
            source: (),
            stage: Stage::PreRecv,
            stats: Stats::default(),
        }
    }
}

impl<C, K: Clone> Clone for Context<C, K> {
    fn clone(&self) -> Self {
        Self {
            book: self.book.clone(),
            addr: self.addr,
            group: self.group,
            demux: self.demux.clone(),
            config: self.config.clone(),
            key: self.key.clone(),
            source: (),
            stage: self.stage,
            stats: Stats::default(),
        }
    }
}

#[must_use]
pub struct RequestBuilder<'c, C, K, S, R, M> {
    context: &'c Context<C, K, S>,
    request: R,
    to: Option<Addr>,
    marker: PhantomData<M>,
}

pub struct Any;
pub struct All;
pub(crate) struct Forgotten;

impl<'c, C, K, S, R> RequestBuilder<'c, C, K, S, R, Any> {
    fn new(context: &'c Context<C, K, S>, request: R) -> Self {
        Self {
            context,
            request,
            to: None,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn all(self) -> RequestBuilder<'c, C, K, S, R, All> {
        RequestBuilder {
            context: self.context,
            request: self.request,
            to: self.to,
            marker: PhantomData,
        }
    }

    // TODO
    #[allow(unused)]
    pub(crate) fn forgotten(self) -> RequestBuilder<'c, C, K, S, R, Forgotten> {
        RequestBuilder {
            context: self.context,
            request: self.request,
            to: self.to,
            marker: PhantomData,
        }
    }
}

impl<'c, C, K, S, R, M> RequestBuilder<'c, C, K, S, R, M> {
    #[deprecated(note = "use `Context::request_to()` instead")]
    #[doc(hidden)]
    #[inline]
    pub fn from(self, addr: Addr) -> Self {
        self.to(addr)
    }

    /// Specified the recipient of the request.
    #[inline]
    fn to(mut self, addr: Addr) -> Self {
        self.to = Some(addr);
        self
    }
}

// TODO: add `pub async fn id() { ... }`
impl<'c, C: 'static, K, S, R: Request> RequestBuilder<'c, C, S, K, R, Any> {
    /// Waits for the response.
    pub async fn resolve(self) -> Result<R::Response, RequestError<R>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.addr;
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
            trace!("< {:?}", envelope.message());
            if let Some(permit) = DUMPER.acquire_m::<R>() {
                permit.record(Dump::message(
                    envelope.message().clone(),
                    envelope.message_kind(),
                    Direction::In,
                ));
            }
            Ok(envelope.into_message().into())
        } else {
            // TODO: should we dump it and increase a counter?
            Err(RequestError::Ignored)
        }
    }
}

impl<'c, C: 'static, K, S, R: Request> RequestBuilder<'c, C, K, S, R, All> {
    /// Waits for the responses.
    pub async fn resolve(self) -> Vec<Result<R::Response, RequestError<R>>> {
        // TODO: cache `OwnedEntry`?
        let this = self.context.addr;
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
                trace!("< {:?}", envelope.message());

                // TODO: `acquire_many` or even unconditionally?
                if let Some(permit) = DUMPER.acquire_m::<R>() {
                    permit.record(Dump::message(
                        envelope.message().clone(),
                        envelope.message_kind(),
                        Direction::In,
                    ));
                }
                Ok(envelope.into_message().into())
            })
            .collect()
    }
}

impl<'c, C: 'static, K, S, R: Request> RequestBuilder<'c, C, S, K, R, Forgotten> {
    pub async fn resolve(self) -> Result<R::Response, RequestError<R>> {
        let token = ResponseToken::forgotten(self.context.book.clone());
        let kind = MessageKind::RequestAny(token);

        let res = if let Some(recipient) = self.to {
            self.context.do_send_to(recipient, self.request, kind).await
        } else {
            self.context.do_send(self.request, kind).await
        };

        if let Err(err) = res {
            return Err(RequestError::Closed(err.0));
        }

        Err(RequestError::Ignored)
    }
}
