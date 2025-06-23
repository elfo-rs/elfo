use std::ops::{Deref, DerefMut, Index};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectTask {
    pub(crate) at: Instant,
    pub(crate) id: ConnId,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Task {
    Connect(ConnectTask),
}

slotmap::new_key_type! {
    struct QueuePlace;
}

type TaskQueue = slotmap::SlotMap<QueuePlace, Task>;

#[derive(Debug)]
pub(crate) struct ConnMan {
    config: config::Config,
    conns: slotmap::SlotMap<ConnId, Conn>,
    task_queue: TaskQueue,
}

impl ConnMan {
    pub(crate) fn new(config: config::Config) -> Self {
        Self {
            config,
            conns: slotmap::SlotMap::<ConnId, _>::default(),
            task_queue: TaskQueue::default(),
        }
    }

    pub(crate) fn drain_task_queue(&mut self) -> impl Iterator<Item = Task> + use<'_> {
        self.task_queue.drain().map(|(_, t)| t)
    }

    fn insert_establishing(&mut self, f: impl FnOnce(QueuePlace) -> Conn) -> ManagedConn<'_> {
        let id = self.conns.insert_with_key(|id| {
            let place = self.task_queue.insert(Task::Connect(ConnectTask {
                at: Instant::MIN,
                id,
            }));
            f(place)
        });

        // SAFETY: just inserted.
        unsafe { self.get_mut(id).unwrap_unchecked() }
    }

    pub(crate) fn insert(
        &mut self,
        role: ConnectionRole,
        transport: ConnectTransport,
    ) -> ManagedConn<'_> {
        self.insert_establishing(move |connect_task| Conn {
            role,
            state: State::Establishing(EstablishingState { connect_task }),
            transport,
        })
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
                task_queue: &mut self.task_queue,
            })
            .ok_or(NoSuchConn(id))
    }

    pub(crate) fn remove(&mut self, id: ConnId) -> Option<Conn> {
        self.conns.remove(id).inspect(|c| {
            if let State::Failed(st) = c.state() {
                self.task_queue.remove(st.reconnect_task);
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
    reconnect_task: QueuePlace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EstablishingState {
    connect_task: QueuePlace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IsVariant)]
pub(crate) enum State {
    Establishing(EstablishingState),
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
    task_queue: &'a mut TaskQueue,
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

impl<'t> StateTransition<'_, 't> {
    fn transition(
        &mut self,
        map: impl FnOnce(&mut ManagedConn<'t>, State) -> Option<State>,
    ) -> Option<State> {
        let prev = self.0.state;
        let new = map(&mut *self.0, prev);

        if new.is_some() {
            match prev {
                State::Establishing(EstablishingState { connect_task }) => {
                    self.0.task_queue.remove(connect_task);
                }
                State::Failed(FailedState { reconnect_task }) => {
                    self.0.task_queue.remove(reconnect_task);
                }

                State::Accepted | State::Established => {}
            }

            new
        } else {
            None
        }
    }

    pub(crate) fn failed(mut self) {
        self.transition(|this, prev| {
            if prev.is_failed() {
                return None;
            }

            let recon_interval = this.config.reconnect_interval;
            let now = Instant::now();
            let retry_at = now.checked_add(recon_interval).unwrap();
            let id = this.id();

            let reconnect_task = this
                .task_queue
                .insert(Task::Connect(ConnectTask { at: retry_at, id }));

            Some(State::Failed(FailedState { reconnect_task }))
        });
    }

    pub(crate) fn establishing(mut self) {
        self.transition(|this, prev| {
            if prev.is_establishing() {
                return None;
            }

            Some(State::Establishing(EstablishingState {
                connect_task: this.task_queue.insert(Task::Connect(ConnectTask {
                    at: Instant::MIN,
                    id: this.id,
                })),
            }))
        });
    }

    pub(crate) fn established(mut self) {
        self.transition(|_, prev| (!prev.is_established()).then_some(State::Established));
    }

    pub(crate) fn accepted(mut self, role: ConnectionRole) {
        self.transition(|this, prev| {
            if prev.is_accepted() {
                return None;
            }

            this.role = role;
            Some(State::Accepted)
        });
    }
}
