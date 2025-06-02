use std::{fmt, ops::Deref, sync::Arc, time::Duration};

use derive_more::{Display, IsVariant};

use tokio::time::Instant;

use elfo_core::message;

use crate::{
    config::Transport,
    node_map::NodeMap,
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

    pub(crate) const fn display(&self) -> impl fmt::Display + use<'_> {
        OptDisplay(match self.direction {
            Direction::Incoming => None,
            Direction::Outgoing => Some(&self.transport),
        })
    }
}

#[derive(Debug, Clone, Copy, Display)]
#[display("no such connection {_0:?}")]
pub(crate) struct NoSuchConn(pub ConnId);

pub(crate) enum EstablishDecision<'a> {
    Proceed(ManagedConnRef<'a>),
    Reject,
}

pub(crate) struct ConnMan {
    config: config::Config,
    conns: slotmap::SlotMap<ConnId, Conn>,
    node_map: Arc<NodeMap>,
}

impl ConnMan {
    pub(crate) fn on_connection_failed(
        &mut self,
        id: ConnId,
    ) -> Result<ManagedConnRef<'_>, NoSuchConn> {
        let conn = self.conns.get_mut(id).ok_or(NoSuchConn(id))?;
        if conn.state.is_failed() {
            return Ok(ManagedConnRef::new(id, conn));
        }

        let recon_interval = self.config.reconnect_interval;
        let now = Instant::now();
        let reconnect_at = now.checked_add(recon_interval).unwrap();

        log_conn!(
            info,
            ManagedConnRef::new(id, conn),
            "connection failed",
            will_reconnect_after = format_args!("{recon_interval:?}"),
        );

        conn.switch_state(id, StateInner::Failed { reconnect_at });

        Ok(ManagedConnRef::new(id, conn))
    }

    pub(crate) fn on_connection_established(
        &mut self,
        id: ConnId,
        info: SocketInfo,
    ) -> Result<EstablishDecision<'_>, NoSuchConn> {
        let conn = self.conns.get(id).ok_or(NoSuchConn(id))?;
        if info.peer.node_no == self.node_map.this.node_no {
            log_conn!(
                info,
                ManagedConnRef::new(id, conn),
                "connection to self ignored"
            );
            // see comment below.
            // drop(conn);
            self.conns.remove(id);

            return Ok(EstablishDecision::Reject);
        }

        // ^^^ Sadly, lack of polonius. This guy refuses to shorten
        // borrow of `self.conns`, because nuh uh we return that reference
        // somewhere below, so, `conns.remove(id)` fails.
        let conn = self.conns.get_mut(id).unwrap();

        match &conn.socket_info {
            Some(prev) if prev != &info => {
                let prev = prev.clone();
                conn.socket_info = Some(info);
                log_conn!(
                    info,
                    ManagedConnRef::new(id, conn),
                    "socket info is changed",
                    transport = conn.transport().display(),
                    prev = prev.raw,
                    prev_capabilities = prev.capabilities
                );
            }
            None => {
                conn.socket_info = Some(info);
            }
            _ => {}
        }

        conn.switch_state(id, StateInner::Established);

        Ok(EstablishDecision::Proceed(ManagedConnRef::new(id, conn)))
    }

    pub(crate) fn on_connection_accepted(
        &mut self,
        id: ConnId,
        role: ConnectionRole,
    ) -> Result<ManagedConnRef<'_>, NoSuchConn> {
        let conn = self.conns.get_mut(id).ok_or(NoSuchConn(id))?;
        conn.role = role;
        conn.switch_state(id, StateInner::Accepted);

        Ok(ManagedConnRef::new(id, conn))
    }

    pub(crate) fn open_connections(
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
                let conn = self.get(id).unwrap();
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
                        self.realloc_id(id).unwrap()
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
}

impl ConnMan {
    pub(crate) fn new(config: config::Config, node_map: Arc<NodeMap>) -> Self {
        Self {
            config,
            conns: slotmap::SlotMap::<ConnId, _>::default(),
            node_map,
        }
    }

    pub(crate) fn insert_new(
        &mut self,
        role: ConnectionRole,
        transport: ConnectTransport,
    ) -> ConnId {
        self.conns.insert(Conn {
            role,
            transport,
            socket_info: None,
            state: StateInner::New {
                connect_at: Instant::now(),
            },
        })
    }

    pub(crate) fn insert_establishing(
        &mut self,
        role: ConnectionRole,
        transport: ConnectTransport,
    ) -> ConnId {
        self.conns.insert(Conn {
            role,
            transport,
            socket_info: None,
            state: StateInner::Establishing,
        })
    }

    pub(crate) fn get(&self, id: ConnId) -> Result<ManagedConnRef<'_>, NoSuchConn> {
        self.conns
            .get(id)
            .map(|conn| ManagedConnRef { id, conn })
            .ok_or(NoSuchConn(id))
    }

    /// Reallocate ID for that connection, previous id would be
    /// no longer accessible.
    fn realloc_id(&mut self, prev: ConnId) -> Result<ConnId, NoSuchConn> {
        let prev = self.conns.remove(prev).ok_or(NoSuchConn(prev))?;
        let id = self.conns.insert(prev);

        Ok(id)
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
    pub(crate) peer: socket::Peer,
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

impl<'a> ManagedConnRef<'a> {
    fn new(id: ConnId, conn: &'a Conn) -> Self {
        Self { id, conn }
    }

    pub(crate) const fn id(&self) -> ConnId {
        self.id
    }
}
