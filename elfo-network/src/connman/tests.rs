use std::{sync::Arc, time::Duration};

use tokio::time::{self, Instant};

use elfo_core::addr::{NodeLaunchId, NodeNo};

use crate::{
    config::Transport,
    connman::{Command, Config, ConnMan, ConnectTransport, EstablishDecision, SocketInfo, Status},
    node_map::NodeMap,
    protocol::ConnectionRole,
    socket,
};

/// Assert that expression matches the pattern. Optionally,
/// part of the pattern could be returned:
/// ```
/// assert_matches!(1, 1);
/// let x = assert_matches!((1, 2), x = (x, _));
/// assert_eq!(x, 1);
/// ```
macro_rules! assert_matches {
    ($e:expr, $pat:pat $(if $guard:expr)? $(,)?) => {
        assert_matches!($e, $pat $(if $guard)? => {})
    };
    ($e:expr, $pat:pat $(if $guard:expr)? => $ret:expr $(,)?) => {{
        // Using `match` for some reason prolongs lifetime of underlying
        // object in $e. For example: `Vec::new().as_slice()` would work here,
        // with let - it wouldn't.
        match $e {
            expr => match expr {
                $pat $(if $guard)? => $ret,
                _ => panic!("{expr:?} does not matches {}", stringify!($pat)),
            },
        }
    }};
}

const RECON_INTERVAL: Duration = Duration::from_millis(100);
const THIS_NODE_NO: u16 = 1;
const THIS_LAUNCH_ID: u64 = 1;

fn manager() -> ConnMan {
    let node_map = Arc::new(NodeMap::empty(
        NodeNo::from_bits(THIS_NODE_NO).unwrap(),
        NodeLaunchId::from_bits(THIS_LAUNCH_ID).unwrap(),
    ));

    ConnMan::new(
        Config {
            reconnect_interval: RECON_INTERVAL,
        },
        node_map,
    )
}

fn tcp_outgoing(host: &str) -> ConnectTransport {
    ConnectTransport::outgoing(Transport::Tcp(format!("{host}:1337")))
}

fn tcp_socket_info(node_no: u16, launch_id: u64) -> SocketInfo {
    let addr = "192.168.0.0:1234".parse().unwrap(); // doesn't matter for now
    let raw = socket::SocketInfo::tcp(addr, addr);
    let peer = socket::Peer {
        node_no: NodeNo::from_bits(node_no).unwrap(),
        launch_id: NodeLaunchId::from_bits(launch_id).unwrap(),
    };
    let capabilities = socket::Capabilities::new(socket::Compression::empty());
    SocketInfo {
        raw,
        peer,
        capabilities,
    }
}

/// Drain connection queue.
#[track_caller]
fn cq_of(man: &mut ConnMan) -> (Option<Duration>, Vec<Command>) {
    let (wake_time, commands) = man.manage_connections();
    (wake_time.map(|t| t - Instant::now()), commands)
}

#[test]
fn it_schedules_opening_for_new_connection() {
    let mut man = manager();
    let conn_id = man.insert_new(ConnectionRole::Control, tcp_outgoing("peer"));
    assert_eq!(man[conn_id].status(), Status::New);

    let (wake, cmds) = cq_of(&mut man);
    assert!(wake.is_none());
    assert_matches!(cmds.as_slice(), [Command::Open(id)] if *id == conn_id);
    assert_eq!(man[conn_id].status(), Status::Establishing);
}

#[tokio::test(start_paused = true)]
async fn it_reconnects_failed_connection() {
    let mut man = manager();
    let prev_id = man.insert_new(ConnectionRole::Control, tcp_outgoing("peer"));
    assert_eq!(man[prev_id].status(), Status::New);

    man.on_connection_failed(prev_id);
    assert_eq!(man[prev_id].status(), Status::Failed);

    // No immediate reconnect.
    let (wake, cmds) = cq_of(&mut man);
    assert_eq!(wake, Some(RECON_INTERVAL));
    assert!(cmds.is_empty());

    time::advance(RECON_INTERVAL).await;

    // Reconnect after a while.
    let (wake, cmds) = cq_of(&mut man);
    assert!(wake.is_none());
    // ID should not be reused.
    assert!(matches!(cmds.as_slice(), [Command::Open(id)] if id != &prev_id));
}

#[test]
fn it_rejects_connection_to_self() {
    let mut man = manager();
    let conn_id = man.insert_new(ConnectionRole::Control, tcp_outgoing("peer"));
    assert_eq!(man[conn_id].status(), Status::New);

    let socket_info = tcp_socket_info(THIS_NODE_NO, 100);
    let decision = man.on_connection_established(conn_id, socket_info).unwrap();
    assert_eq!(decision, EstablishDecision::Reject);
    assert!(man.get(conn_id).is_none());
}

#[test]
fn it_accepts_connection() {
    let mut man = manager();
    let conn_id = man.insert_establishing(ConnectionRole::Control, tcp_outgoing("peer"));
    assert_eq!(man[conn_id].status(), Status::Establishing);

    let socket_info = tcp_socket_info(2, 100);
    let decision = man.on_connection_established(conn_id, socket_info).unwrap();
    assert_eq!(decision, EstablishDecision::Proceed);
    assert_eq!(man[conn_id].status(), Status::Established);
    assert!(man[conn_id].socket_info().is_some());

    man.on_connection_accepted(conn_id, ConnectionRole::Control);
    assert_eq!(man[conn_id].status(), Status::Accepted);
}

#[test]
fn it_aborts_connections() {
    let mut man = manager();

    // A connection that shouldn't be aborted.
    let good_id = man.insert_establishing(ConnectionRole::Control, tcp_outgoing("good"));
    assert_eq!(man[good_id].status(), Status::Establishing);

    // A failed connection that should be removed immediately when aborted.
    let failed_id = man.insert_establishing(ConnectionRole::Control, tcp_outgoing("bad"));
    assert_eq!(man[failed_id].status(), Status::Establishing);
    man.on_connection_failed(failed_id);
    assert_eq!(man[failed_id].status(), Status::Failed);

    // A failed connection that should be switched to `Aborting` until failed.
    let establishing_id = man.insert_establishing(ConnectionRole::Control, tcp_outgoing("bad"));
    assert_eq!(man[establishing_id].status(), Status::Establishing);

    let aborted = man.abort_by_transport(&"tcp://bad:1337".parse().unwrap());
    assert_eq!(aborted, &[establishing_id]);
    assert_eq!(man[establishing_id].status(), Status::Aborting);
    // Failed connections are removed immediately.
    assert!(man.get(failed_id).is_none());
    assert_eq!(man[good_id].status(), Status::Establishing);

    // Finally the last connection fails and removed immediately.
    man.on_connection_failed(establishing_id);
    assert!(man.get(establishing_id).is_none());
}
