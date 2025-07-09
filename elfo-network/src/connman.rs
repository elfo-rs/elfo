use std::{
    fmt,
    ops::{Deref, DerefMut},
    time::Duration,
};

use derive_more::{Display, IsVariant};

use tokio::time::Instant;

use elfo_core::message;

use crate::{
    config::Transport,
    protocol::{ConnId, ConnectionRole},
    socket,
};

pub(crate) use self::config::Config;

#[doc(hidden)]
pub(crate) struct OptDisplay<'a, T: ?Sized>(pub(crate) Option<&'a T>);

impl<T: fmt::Display + ?Sized> fmt::Display for OptDisplay<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(t) = self.0 {
            fmt::Display::fmt(t, f)
        } else {
            f.write_str("<null>")
        }
    }
}

/// Log something exposing structurally all connection details.
macro_rules! _log {
    ($level:ident, $conn:expr, $text:literal $(, $($label:ident = $value:expr),* $(,)?)?) => {{
        match $conn {
            ref conn => {
                tracing::$level!(
                    role = %conn.role(),
                    transport = %conn.transport(),
                    id = %conn.id(),
                    state = %conn.state(),
                    socket = %$crate::connman::OptDisplay(conn.socket_info().map(|s| &s.raw)),
                    capabilities = %$crate::connman::OptDisplay(conn.socket_info().map(|s| &s.capabilities)),
                    $( $( $label = %$value, )* )?
                    message = $text,
                )
            }
        }
    }};
}

pub(crate) use _log as log_conn;

#[cfg(test)]
mod tests;

mod config;

#[message(part)]
#[derive(Display, IsVariant)]
pub(crate) enum Direction {
    #[display("incoming")]
    Incoming,
    #[display("outgoing")]
    Outgoing,
}

#[message(part)]
#[derive(Display)]
#[display("{direction}#{transport}")]
pub(crate) struct ConnectTransport {
    /// Transport using which connection was made.
    pub(crate) transport: Transport,
    /// To what transport refers.
    /// - [`Direction::Incoming`] - remote node connected to us
    /// - [`Direction::Outgoing`] - we connected to the remote node
    pub(crate) direction: Direction,
}

impl ConnectTransport {
    pub(crate) const fn outgoing(transport: Transport) -> Self {
        Self {
            transport,
            direction: Direction::Outgoing,
        }
    }

    pub(crate) const fn incoming(transport: Transport) -> Self {
        Self {
            transport,
            direction: Direction::Incoming,
        }
    }
}

#[derive(Debug, Clone, Copy, Display)]
#[display("no such connection {_0:?}")]
pub(crate) struct NoSuchConn(pub ConnId);

pub(crate) struct ConnMan {
    config: config::Config,
    conns: slotmap::SlotMap<ConnId, Conn>,
}

impl ConnMan {
    pub(crate) fn new(config: config::Config) -> Self {
        Self {
            config,
            conns: slotmap::SlotMap::<ConnId, _>::default(),
        }
    }

    pub(crate) fn make_connections(
        &mut self,
    ) -> (Option<Instant>, impl Iterator<Item = (State, ConnId)>) {
        let now = Instant::now();
        let mut least_time: Option<Instant> = None;
        let mut ready = Vec::new();

        for (id, conn) in self.conns.iter_mut() {
            // Let's think about it as a closure.
            macro_rules! establishing {
                ($time:expr) => {{
                    let time = $time;
                    let diff = time.duration_since(now);
                    // tokio timers are 1ms precise anyway.
                    if diff <= Duration::from_millis(1) {
                        conn.socket_info = None;
                        let prev = conn.switch_state(id, StateInner::Establishing);
                        ready.push((State(prev), id));
                    } else if let Some(least) = &mut least_time {
                        *least = (*least).min(time);
                    } else {
                        least_time = Some(time);
                    }
                }};
            }

            match conn.state {
                StateInner::Failed { reconnect_at } => {
                    establishing!(reconnect_at)
                }
                StateInner::New { connect_at } => establishing!(connect_at),
                _ => {}
            }
        }

        let ready = ready
            .into_iter()
            .filter_map(|(st, id)| {
                let conn = self.get_mut(id).unwrap();
                if conn.transport().direction.is_incoming() {
                    log_conn!(
                        info,
                        conn,
                        "can't connect to node which transport is unknown, remote node must connect",
                    );
                    _ = self.conns.remove(id).unwrap();

                    return None;
                }

                let id = match st.0 {
                    StateInner::Failed { .. } => {
                        self.realloc_id(id).unwrap().id()
                    },
                    _ => id,
                };

                Some((st, id))
            })
            // Simplest way of draining whole iterator, it's needed for
            // ID reallocation in case of accidental iterator drop.
            .collect::<Vec<_>>();
        (least_time, ready.into_iter())
    }

    pub(crate) fn insert_new(
        &mut self,
        role: ConnectionRole,
        transport: ConnectTransport,
    ) -> ManagedConnMut<'_> {
        let id = self.conns.insert(Conn {
            role,
            transport,
            socket_info: None,
            state: StateInner::New {
                connect_at: Instant::now(),
            },
        });
        self.get_mut(id).expect("just inserted")
    }

    pub(crate) fn insert_establishing(
        &mut self,
        role: ConnectionRole,
        transport: ConnectTransport,
    ) -> ManagedConnMut<'_> {
        let id = self.conns.insert(Conn {
            role,
            transport,
            socket_info: None,
            state: StateInner::Establishing,
        });

        self.get_mut(id).expect("just inserted")
    }

    pub(crate) fn get(&self, id: ConnId) -> Result<ManagedConnRef<'_>, NoSuchConn> {
        self.conns
            .get(id)
            .map(|conn| ManagedConnRef { id, conn })
            .ok_or(NoSuchConn(id))
    }

    pub(crate) fn get_mut(&mut self, id: ConnId) -> Result<ManagedConnMut<'_>, NoSuchConn> {
        self.conns
            .get_mut(id)
            .map(|conn| ManagedConnMut {
                id,
                conn,
                config: &self.config,
            })
            .ok_or(NoSuchConn(id))
    }

    /// Reallocate ID for that connection, previous id would be
    /// no longer accessible.
    fn realloc_id(&mut self, prev: ConnId) -> Result<ManagedConnMut<'_>, NoSuchConn> {
        let prev = self.conns.remove(prev).ok_or(NoSuchConn(prev))?;
        let id = self.conns.insert(prev);

        Ok(self.get_mut(id).expect("just inserted"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IsVariant, derive_more::Display)]
enum StateInner {
    #[display("New")]
    New {
        connect_at: Instant,
    },
    Establishing,
    Established,
    Accepted,
    #[display("Failed")]
    Failed {
        reconnect_at: Instant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub(crate) struct State(StateInner);

#[derive(PartialEq, Eq, Clone)]
pub(crate) struct SocketInfo {
    pub(crate) raw: socket::SocketInfo,
    pub(crate) capabilities: socket::Capabilities,
}

pub(crate) struct Conn {
    role: ConnectionRole,
    state: StateInner,
    transport: ConnectTransport,
    socket_info: Option<SocketInfo>,
}

impl Conn {
    pub(crate) const fn transport(&self) -> &ConnectTransport {
        &self.transport
    }

    pub(crate) const fn role(&self) -> ConnectionRole {
        self.role
    }

    pub(crate) const fn state(&self) -> State {
        State(self.state)
    }

    pub(crate) fn socket_info(&self) -> Option<&SocketInfo> {
        self.socket_info.as_ref()
    }

    fn switch_state(&mut self, id: ConnId, new: StateInner) -> StateInner {
        let prev = std::mem::replace(&mut self.state, new);
        log_conn!(
            debug,
            ManagedConnRef { id, conn: &*self },
            "connection is switched state",
            from = prev,
        );
        prev
    }
}

pub(crate) struct ManagedConnRef<'a> {
    id: ConnId,
    conn: &'a Conn,
}

impl Deref for ManagedConnRef<'_> {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        self.conn
    }
}

impl ManagedConnRef<'_> {
    pub(crate) const fn id(&self) -> ConnId {
        self.id
    }
}

pub(crate) struct ManagedConnMut<'a> {
    id: ConnId,
    config: &'a Config,
    conn: &'a mut Conn,
}

impl<'a> ManagedConnMut<'a> {
    pub(crate) const fn id(&self) -> ConnId {
        self.id
    }

    pub(crate) fn change_state<'this>(&'this mut self, f: impl FnOnce(StateTransition<'this, 'a>)) {
        f(StateTransition(self))
    }
}

impl Deref for ManagedConnMut<'_> {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &*self.conn
    }
}

impl DerefMut for ManagedConnMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.conn
    }
}

pub(crate) struct StateTransition<'a, 't>(&'a mut ManagedConnMut<'t>);

impl<'t> StateTransition<'_, 't> {
    fn transition(
        &mut self,
        map: impl FnOnce(&mut ManagedConnMut<'t>, StateInner) -> Option<StateInner>,
    ) {
        let prev = self.0.state;
        let id = self.0.id;

        if let Some(next) = map(&mut *self.0, prev) {
            self.0.switch_state(id, next);
        }
    }

    pub(crate) fn failed(mut self) {
        self.transition(|this, prev| {
            if prev.is_failed() {
                return None;
            }

            let recon_interval = this.config.reconnect_interval;
            let now = Instant::now();
            let reconnect_at = now.checked_add(recon_interval).unwrap();

            Some(StateInner::Failed { reconnect_at })
        });
    }

    pub(crate) fn established(mut self, info: SocketInfo) {
        self.transition(|this, prev_state| {
            match this.socket_info() {
                Some(prev) if prev != &info => {
                    let prev = prev.clone();
                    this.socket_info = Some(info);
                    log_conn!(
                        info,
                        this,
                        "socket info is changed",
                        prev = prev.raw,
                        prev_capabilities = prev.capabilities
                    );
                }
                None => {
                    this.socket_info = Some(info);
                }
                _ => {}
            };

            (!prev_state.is_established()).then_some(StateInner::Established)
        });
    }

    pub(crate) fn accepted(mut self, role: ConnectionRole) {
        self.transition(|this, prev| {
            if prev.is_accepted() {
                return None;
            }

            this.role = role;
            Some(StateInner::Accepted)
        });
    }
}
