use std::{fmt, ops::Index, sync::Arc};

use derive_more::{Display, IsVariant};
use slotmap::SlotMap;
use tokio::time::Instant;

use elfo_core::message;
use elfo_utils::ward;

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
                    message = $text,
                    role = %conn.role(),
                    id = %conn.id(),
                    status = %conn.status(),
                    socket = %$crate::connman::OptDisplay(conn.socket_info().map(|s| &s.raw)),
                    capabilities = %$crate::connman::OptDisplay(conn.socket_info().map(|s| &s.capabilities)),
                    $( $( $label = %$value, )* )?
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EstablishDecision {
    Proceed,
    Reject,
}

#[derive(Debug)]
pub(crate) enum Command {
    Open(ConnId),
}

pub(crate) struct ConnMan {
    config: config::Config,
    node_map: Arc<NodeMap>,
    conns: SlotMap<ConnId, Conn>,
}

// TODO: revise logging.

impl ConnMan {
    pub(crate) fn new(config: config::Config, node_map: Arc<NodeMap>) -> Self {
        Self {
            config,
            node_map,
            conns: SlotMap::<ConnId, _>::default(),
        }
    }

    pub(crate) fn insert_new(
        &mut self,
        role: ConnectionRole,
        transport: ConnectTransport,
    ) -> ConnId {
        self.conns.insert_with_key(|conn_id| Conn {
            id: conn_id,
            role,
            transport,
            socket_info: None,
            state: State::New {
                connect_at: Instant::now(),
            },
        })
    }

    pub(crate) fn insert_establishing(
        &mut self,
        role: ConnectionRole,
        transport: ConnectTransport,
    ) -> ConnId {
        self.conns.insert_with_key(|conn_id| Conn {
            id: conn_id,
            role,
            transport,
            socket_info: None,
            state: State::Establishing,
        })
    }

    pub(crate) fn abort_by_transport(&mut self, transport: &Transport) -> Vec<ConnId> {
        let mut conn_ids = Vec::new();

        self.conns.retain(|id, conn| {
            if conn.status().is_aborting() || &conn.transport.transport != transport {
                return true;
            }

            // Failed connections can be removed immediately.
            if conn.status().is_failed() {
                return false;
            }

            conn.switch_state(State::Aborting);
            conn_ids.push(id);
            true
        });

        conn_ids
    }

    pub(crate) fn get(&self, id: ConnId) -> Option<&Conn> {
        self.conns.get(id)
    }

    pub(crate) fn manage_connections(&mut self) -> (Option<Instant>, Vec<Command>) {
        let now = Instant::now();
        let mut least_time: Option<Instant> = None;
        let mut commands = Vec::new();

        for conn_id in self.conns.keys().collect::<Vec<_>>() {
            let (wake_time, command) = self.manage_connection(conn_id, now);

            // Choose the least time to wake up next among all connections.
            least_time = match (least_time, wake_time) {
                (Some(least), Some(wake)) => Some(least.min(wake)),
                (least, wake) => least.or(wake),
            };

            if let Some(command) = command {
                commands.push(command);
            }
        }

        (least_time, commands)
    }

    fn manage_connection(
        &mut self,
        conn_id: ConnId,
        now: Instant,
    ) -> (Option<Instant>, Option<Command>) {
        let conn = self.conns.get_mut(conn_id).expect("expected to exist");

        match &conn.state {
            // New -> Establishing
            State::New { connect_at } if *connect_at <= now => {
                debug_assert!(conn.transport().direction.is_outgoing());
                debug_assert!(conn.socket_info.is_none());
                conn.switch_state(State::Establishing);
                (None, Some(Command::Open(conn_id)))
            }
            State::New { connect_at } => (Some(*connect_at), None),

            // Failed -> <removed>
            // <new> -> Establishing
            State::Failed { reconnect_at } if *reconnect_at <= now => {
                debug_assert!(conn.transport().direction.is_outgoing());
                let old = self.conns.remove(conn_id).unwrap();
                let conn_id = self.insert_establishing(old.role, old.transport);
                (None, Some(Command::Open(conn_id)))
            }
            State::Failed { reconnect_at } => (Some(*reconnect_at), None),

            _ => (None, None),
        }
    }
}

// TODO: forbid more transitions if `Aborting`

impl ConnMan {
    pub(crate) fn on_connection_failed(&mut self, id: ConnId) {
        let conn = ward!(self.conns.get_mut(id));
        if conn.status().is_failed() {
            return;
        }

        if conn.status().is_aborting() {
            log_conn!(info, conn, "connection aborted");
            self.conns.remove(id);
            return;
        }

        let recon_interval = self.config.reconnect_interval;
        let now = Instant::now();
        let reconnect_at = now.checked_add(recon_interval).unwrap();

        log_conn!(
            info,
            conn,
            "connection failed",
            will_reconnect_after = format_args!("{recon_interval:?}"),
        );

        if conn.transport().direction.is_incoming() {
            log_conn!(
                info,
                conn,
                "can't connect to node which transport is unknown, remote node must connect",
            );
            self.conns.remove(id);
        } else {
            conn.switch_state(State::Failed { reconnect_at });
        }
    }

    pub(crate) fn on_connection_established(
        &mut self,
        id: ConnId,
        info: SocketInfo,
    ) -> Result<EstablishDecision, NoSuchConn> {
        let conn = self.conns.get_mut(id).ok_or(NoSuchConn(id))?;

        if info.peer.node_no == self.node_map.this.node_no {
            log_conn!(info, conn, "connection to self ignored");
            self.conns.remove(id);
            return Ok(EstablishDecision::Reject);
        }

        match &conn.socket_info {
            Some(prev) if prev != &info => {
                let prev = prev.clone();
                conn.socket_info = Some(info);
                log_conn!(
                    info,
                    conn,
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

        conn.switch_state(State::Established);

        Ok(EstablishDecision::Proceed)
    }

    pub(crate) fn on_connection_accepted(&mut self, id: ConnId, role: ConnectionRole) {
        let conn = ward!(self.conns.get_mut(id));
        // TODO: role can be changed only if it was `Unknown` before.
        conn.role = role;
        conn.switch_state(State::Accepted);
    }
}

impl Index<ConnId> for ConnMan {
    type Output = Conn;

    #[track_caller]
    fn index(&self, index: ConnId) -> &Self::Output {
        self.conns.get(index).expect("no such connection")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IsVariant, Display)]
pub(crate) enum Status {
    New,
    Establishing,
    Established,
    Accepted,
    Failed,
    Aborting,
}

#[derive(Debug)]
enum State {
    New { connect_at: Instant },
    Establishing,
    Established,
    Accepted,
    Failed { reconnect_at: Instant },
    Aborting,
}

#[derive(PartialEq, Eq, Clone)]
pub(crate) struct SocketInfo {
    pub(crate) raw: socket::SocketInfo,
    pub(crate) peer: socket::Peer,
    pub(crate) capabilities: socket::Capabilities,
}

pub(crate) struct Conn {
    id: ConnId,
    role: ConnectionRole,
    state: State,
    transport: ConnectTransport,
    socket_info: Option<SocketInfo>,
}

impl Conn {
    pub(crate) const fn id(&self) -> ConnId {
        self.id
    }

    pub(crate) const fn transport(&self) -> &ConnectTransport {
        &self.transport
    }

    pub(crate) const fn role(&self) -> ConnectionRole {
        self.role
    }

    pub(crate) const fn status(&self) -> Status {
        match &self.state {
            State::New { .. } => Status::New,
            State::Establishing => Status::Establishing,
            State::Established => Status::Established,
            State::Accepted => Status::Accepted,
            State::Failed { .. } => Status::Failed,
            State::Aborting => Status::Aborting,
        }
    }

    pub(crate) fn socket_info(&self) -> Option<&SocketInfo> {
        self.socket_info.as_ref()
    }

    fn switch_state(&mut self, new: State) {
        let prev_status = self.status();
        self.state = new;
        log_conn!(debug, self, "connection switched state", from = prev_status);
    }
}
