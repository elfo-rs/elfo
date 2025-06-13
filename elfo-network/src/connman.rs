use std::ops::{Deref, DerefMut, Index};

use derive_more::Display;

use crate::{config::Transport, protocol::ConnectionRole};

#[derive(Debug, Clone, Copy, Display)]
#[display("no such connection {_0:?}")]
pub(crate) struct NoSuchConn(pub ConnId);

slotmap::new_key_type! {
    pub struct ConnId;
}

pub(crate) struct Failed<'a>(&'a mut ConnMan);

impl<'a> Failed<'a> {
    /// Get number of failed connections.
    pub(crate) fn len(&self) -> usize {
        self.0.failed.len()
    }

    /// Pop one failed connection for reconnection purpose. Automatically
    /// sets state to establishing.
    pub(crate) fn pop_for_establishing(&mut self) -> Option<(ConnId, ManagedConn<'_>)> {
        let id = self.0.failed.keys().next()?;
        _ = self.0.failed.remove(id);

        let mut conn = self.0.get_mut(id).unwrap();
        conn.change_state(|t| t.establishing());

        Some((id, conn))
    }
}

#[derive(Debug, Default)]
pub(crate) struct ConnMan {
    conns: slotmap::SlotMap<ConnId, Conn>,
    failed: slotmap::SecondaryMap<ConnId, ()>,
}

impl ConnMan {
    pub(crate) fn insert(&mut self, conn: Conn) -> ConnId {
        self.conns.insert(conn)
    }

    pub(crate) const fn failed(&mut self) -> Failed<'_> {
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
                failed: &mut self.failed,
            })
            .ok_or(NoSuchConn(id))
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
pub(crate) enum State {
    Establishing,
    Established,
    Accepted,
    Failed,
}

#[derive(Debug)]
pub(crate) struct Conn {
    role: ConnectionRole,
    state: State,
    transport: Option<Transport>,
}

impl Conn {
    pub(crate) fn new(role: ConnectionRole, transport: Transport) -> Self {
        Self {
            role,
            transport: Some(transport),
            state: State::Establishing,
        }
    }

    pub(crate) const fn transport(&self) -> Option<&Transport> {
        self.transport.as_ref()
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
    conn: &'a mut Conn,
    failed: &'a mut slotmap::SecondaryMap<ConnId, ()>,
}

impl<'a> ManagedConn<'a> {
    pub(crate) fn change_state<'this>(&'this mut self, f: impl FnOnce(StateTransition<'this, 'a>)) {
        f(StateTransition(self))
    }
}

impl<'a> Deref for ManagedConn<'a> {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &*self.conn
    }
}

impl<'a> DerefMut for ManagedConn<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.conn
    }
}

pub(crate) struct StateTransition<'a, 't>(&'a mut ManagedConn<'t>);

impl<'a, 't> StateTransition<'a, 't> {
    fn transition(&mut self, to: State) -> Option<State> {
        let prev = self.0.state;
        if to != prev {
            self.0.state = to;
            Some(prev)
        } else {
            None
        }
    }

    pub(crate) fn failed(mut self) {
        if self.transition(State::Failed).is_some() {
            self.0.failed.insert(self.0.id, ());
        }
    }

    pub(crate) fn establishing(mut self) {
        self.transition(State::Establishing);
    }

    pub(crate) fn established(mut self) {
        self.transition(State::Establishing);
    }

    pub(crate) fn accepted(mut self, role: ConnectionRole) {
        if self.transition(State::Accepted).is_some() {
            // self.0.role = role;
        }
    }
}
