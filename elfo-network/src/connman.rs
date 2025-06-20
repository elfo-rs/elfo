use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut, Index},
    time::Duration,
};

use derive_more::{Display, IsVariant};

use elfo_core::message;
use elfo_utils::time::Instant;

use crate::{config::Transport, protocol::ConnectionRole};

pub(crate) use self::config::Config;

#[message(part)]
#[derive(Display, IsVariant)]
pub(crate) enum TransportKind {
    /// This transport refers to the remote node.
    #[display("remote")]
    Remote,
    /// This transport refers to the current node.
    #[display("current_node")]
    CurrentNode,
}

#[message(part)]
#[derive(Display)]
#[display("{kind}#{transport}")]
pub(crate) struct ConnectTransport {
    /// Transport using which connection was made.
    pub(crate) transport: Transport,
    /// To what transport refers.
    /// - [`TransportKind::Remote`] - to the remote node, connection
    /// initiator is current node
    /// - [`TransportKind::CurrentNode`] - to the current node, connection
    /// initiator is the remote node.
    pub(crate) kind: TransportKind,
}

impl ConnectTransport {
    pub(crate) const fn remote(transport: Transport) -> Self {
        Self {
            transport,
            kind: TransportKind::Remote,
        }
    }

    pub(crate) const fn current_node(transport: Transport) -> Self {
        Self {
            transport,
            kind: TransportKind::CurrentNode,
        }
    }
}

#[cfg(test)]
mod tests;

mod config;

#[derive(Debug, Clone, Copy, Display)]
#[display("no such connection {_0:?}")]
pub(crate) struct NoSuchConn(pub ConnId);

slotmap::new_key_type! {
    #[derive(Display)]
    #[display("conn:{:X}", self.0.as_ffi())]
    pub struct ConnId;
}

#[message(part)]
#[derive(PartialEq, Eq)]
pub(crate) struct DeferReconnect {
    pub(crate) after: Duration,
    pub(crate) id: ConnId,
}

pub(crate) struct Failed<'a>(&'a mut ConnMan);

impl Failed<'_> {
    /// Pop one from retry queue.
    pub(crate) fn pop_for_retry(&mut self) -> Result<ConnId, Option<DeferReconnect>> {
        let current_time = Instant::now();
        let (QueueKey { at, .. }, id) = self
            .0
            .retry_schedule
            .queue
            .first_entry()
            .ok_or(None)?
            .remove_entry();

        if current_time >= at {
            Ok(id)
        } else {
            Err(Some(DeferReconnect {
                after: at.duration_since(current_time),
                id,
            }))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct QueueKey {
    at: Instant,
    nth: usize,
}

impl QueueKey {
    const fn new(at: Instant, nth: usize) -> Self {
        Self { at, nth }
    }
}

#[derive(Debug, Default)]
struct RetrySchedule {
    queue: BTreeMap<QueueKey, ConnId>,
}

#[derive(Debug)]
pub(crate) struct ConnMan {
    config: config::Config,
    conns: slotmap::SlotMap<ConnId, Conn>,
    retry_schedule: RetrySchedule,
}

impl ConnMan {
    pub(crate) fn new(config: config::Config) -> Self {
        Self {
            config,
            conns: slotmap::SlotMap::<_, _>::default(),
            retry_schedule: RetrySchedule::default(),
        }
    }

    pub(crate) fn insert(&mut self, conn: Conn) -> ManagedConn<'_> {
        let id = self.conns.insert(conn);
        // SAFETY: just got inserted.
        unsafe { self.get_mut(id).unwrap_unchecked() }
    }

    pub(crate) fn failed(&mut self) -> Failed<'_> {
        Failed(self)
    }

    pub(crate) fn get(&self, id: ConnId) -> Result<&Conn, NoSuchConn> {
        self.conns.get(id).ok_or(NoSuchConn(id))
    }

    pub(crate) fn get_mut(&mut self, id: ConnId) -> Result<ManagedConn<'_>, NoSuchConn> {
        self.conns
            .get_mut(id)
            .map(|conn| ManagedConn {
                id,
                conn,
                config: &self.config,
                retry_schedule: &mut self.retry_schedule,
            })
            .ok_or(NoSuchConn(id))
    }

    pub(crate) fn remove(&mut self, id: ConnId) -> Option<Conn> {
        self.conns.remove(id).inspect(|c| {
            if let State::Failed(st) = c.state() {
                self.retry_schedule.queue.remove(&st.queue_place);
            }
        })
    }
}

impl Index<ConnId> for ConnMan {
    type Output = Conn;

    #[track_caller]
    fn index(&self, index: ConnId) -> &Self::Output {
        self.get(index).unwrap()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailedState {
    queue_place: QueueKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IsVariant)]
pub(crate) enum State {
    Establishing,
    Established,
    Accepted,
    Failed(FailedState),
}

#[derive(Debug)]
pub(crate) struct Conn {
    role: ConnectionRole,
    state: State,
    transport: ConnectTransport,
}

impl Conn {
    pub(crate) fn new(role: ConnectionRole, transport: ConnectTransport) -> Self {
        Self {
            role,
            transport,
            state: State::Establishing,
        }
    }

    pub(crate) const fn transport(&self) -> &ConnectTransport {
        &self.transport
    }

    pub(crate) const fn role(&self) -> ConnectionRole {
        self.role
    }

    pub(crate) const fn state(&self) -> State {
        self.state
    }
}

pub(crate) struct ManagedConn<'a> {
    id: ConnId,
    config: &'a Config,
    conn: &'a mut Conn,
    retry_schedule: &'a mut RetrySchedule,
}

impl<'a> ManagedConn<'a> {
    pub(crate) const fn id(&self) -> ConnId {
        self.id
    }

    pub(crate) fn change_state<'this>(&'this mut self, f: impl FnOnce(StateTransition<'this, 'a>)) {
        f(StateTransition(self))
    }
}

impl Deref for ManagedConn<'_> {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &*self.conn
    }
}

impl DerefMut for ManagedConn<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.conn
    }
}

pub(crate) struct StateTransition<'a, 't>(&'a mut ManagedConn<'t>);

impl StateTransition<'_, '_> {
    fn transition(&mut self, to: State) -> Option<State> {
        use std::mem::discriminant;

        let prev = self.0.state;

        // Comparing discriminants to ignore state
        // data.
        if discriminant(&to) != discriminant(&prev) {
            // If transitioning `Failed -> !Failed`, let's remove
            // ourselves from schedule.
            if let State::Failed(st) = prev {
                self.0.retry_schedule.queue.remove(&st.queue_place);
            }

            self.0.state = to;
            Some(prev)
        } else {
            None
        }
    }

    pub(crate) fn failed(self) {
        if self.0.state.is_failed() {
            return;
        }

        let recon_interval = self.0.config.reconnect_interval;
        let now = Instant::now();
        let retry_at = now.checked_add(recon_interval).unwrap();
        let id = self.0.id();

        let nth = self
            .0
            .retry_schedule
            .queue
            .range(QueueKey::new(retry_at, 0)..QueueKey::new(retry_at, usize::MAX))
            .next_back()
            .map_or(0, |(QueueKey { nth, .. }, _)| nth + 1);
        let queue_place = QueueKey::new(retry_at, nth);
        self.0.retry_schedule.queue.insert(queue_place, id);

        self.0.state = State::Failed(FailedState { queue_place });
    }

    pub(crate) fn establishing(mut self) {
        self.transition(State::Establishing);
    }

    pub(crate) fn established(mut self) {
        self.transition(State::Established);
    }

    pub(crate) fn accepted(mut self, role: ConnectionRole) {
        if self.transition(State::Accepted).is_some() {
            self.0.role = role;
        }
    }
}
