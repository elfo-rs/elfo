use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut, Index},
    time::Duration,
};

use derive_more::Display;

use elfo_utils::time::Instant;
use tracing::debug;

use crate::{config::Transport, protocol::ConnectionRole};

pub(crate) use self::config::Config;

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

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NextCheckAdvise {
    pub(crate) duration: Duration,
}

pub(crate) struct Failed<'a>(&'a mut ConnMan);

impl Failed<'_> {
    /// Pop one failed connection for reconnection purpose. Automatically
    /// sets state to establishing.
    pub(crate) fn pop_for_establishing(&mut self) -> Result<ConnId, Option<NextCheckAdvise>> {
        // IMO allowing to customize this is redundant. Making `PRECISION` larger
        // can lessen latency for reconnection, but can also produce more
        // "spurious" reconnects, which can affect latency as well, especially
        // when backoff would be introduced. When adjusting this constant, make wise
        // decision.
        const PRECISION: Duration = Duration::from_millis(2);

        let cur = Instant::now();
        loop {
            let entry = self.0.retry_schedule.first_entry().ok_or(None)?;
            let after = *entry.key();

            // if current time is in `[after - PRECISION; after]`, then
            // pass further, otherwise advise to wait.
            if cur < after && after.duration_since(cur) < PRECISION {
                return Err(Some(NextCheckAdvise {
                    duration: after.duration_since(cur),
                }));
            }

            let id = entry.remove();
            let mut conn = self.0.get_mut(id).unwrap();
            let state = conn.state();

            // Unlikely, but let's log that occasion, this signals about bug
            // in state handling I believe.
            if !matches!(state, State::Failed) {
                debug!(state = ?state, id = %id, "got non-failed connection in the retry schedule");
                continue;
            }

            conn.change_state(|t| t.establishing());
            return Ok(id);
        }
    }
}

#[derive(Debug)]
pub(crate) struct ConnMan {
    config: config::Config,
    conns: slotmap::SlotMap<ConnId, Conn>,
    retry_schedule: BTreeMap<Instant, ConnId>,
}

impl ConnMan {
    pub(crate) fn new(config: config::Config) -> Self {
        Self {
            config,
            conns: slotmap::SlotMap::<_, _>::default(),
            retry_schedule: BTreeMap::default(),
        }
    }

    pub(crate) fn insert(&mut self, conn: Conn) -> ManagedConn<'_> {
        let id = self.conns.insert(conn);
        // SAFETY: just got inserted.
        unsafe { self.get_mut(id).unwrap_unchecked() }
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
                config: &self.config,
                retry_schedule: &mut self.retry_schedule,
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
    transport: Transport,
}

impl Conn {
    pub(crate) fn new(role: ConnectionRole, transport: Transport) -> Self {
        Self {
            role,
            transport,
            state: State::Establishing,
        }
    }

    pub(crate) const fn transport(&self) -> &Transport {
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
    retry_schedule: &'a mut BTreeMap<Instant, ConnId>,
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
            self.0.state = to;
            Some(prev)
        } else {
            None
        }
    }

    pub(crate) fn failed(mut self) {
        if self.transition(State::Failed).is_some() {
            let recon_interval = self.0.config.reconnect_interval;
            let now = Instant::now();

            let mut retry_at = now.checked_add(recon_interval).unwrap();
            let mut id = self.0.id;
            let _1ns = Duration::from_nanos(1);

            // It's highly unlikely that we get the same nanosecond instant,
            // but just to not screw everything up - let's make things simple
            // and just add 1ns if we met same instant by the fortune.
            while let Some(prev) = self.0.retry_schedule.insert(retry_at, id) {
                id = prev;
                retry_at = retry_at.checked_add(_1ns).unwrap();
            }
        }
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
