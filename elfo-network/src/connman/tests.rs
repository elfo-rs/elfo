use std::time::Duration;
use tokio::time;

use crate::{
    config::Transport,
    connman::{Config, ConnId, ConnMan, ConnectTransport, StateInner},
    node_map::NodeMap,
    protocol::ConnectionRole,
};

const RECON_INTERVAL: Duration = Duration::from_millis(100);

/// Assert that expression matches the pattern. Optionally,
/// part of the pattern could be returned:
/// ```
/// use elfo_utils::assert_matches;
///
/// assert_matches!(1, 1);
/// let x = assert_matches!((1, 2), x = (x, _));
/// assert_eq!(x, 1);
/// ```
macro_rules! assert_matches {
    ($e:expr, $pat:pat $(,)?) => {
        assert_matches!($e, $pat => {})
    };
    ($e:expr, $pat:pat => $ret:expr $(,)?) => {{
        // Using `match` for some reason prolongs lifetime of underlying
        // object in $e. For example: `Vec::new().as_slice()` would work here,
        // with let - it wouldn't.
        match $e {
            expr => match expr {
                $pat => $ret,
                _ => panic!("{expr:?} does not matches {}", stringify!($pat)),
            },
        }
    }};
}

fn manager() -> ConnMan {
    ConnMan::new(
        Config {
            reconnect_interval: RECON_INTERVAL,
        },
        NodeMap::test().into(),
    )
}

fn tcp() -> ConnectTransport {
    ConnectTransport::outgoing(Transport::Tcp("0.0.0.0:1337".to_owned()))
}

fn control() -> ConnectionRole {
    ConnectionRole::Control
}

/// drain connection queue.
#[track_caller]
fn cq_of(man: &mut ConnMan) -> Vec<(StateInner, ConnId)> {
    let (_, cq) = man.open_connections();

    cq.map(|(st, id)| (st.0, id)).collect()
}

#[tokio::test(start_paused = true)]
async fn inserting_new_adds_to_connection_queue_immediately() {
    let mut man = manager();
    let expected = man.insert_new(control(), tcp());
    let got = assert_matches!(cq_of(&mut man).as_slice(), [(StateInner::New { .. }, id)] => *id);

    assert_eq!(expected, got);
}

#[tokio::test(start_paused = true)]
async fn failed_connections_reconnect_after_delay() {
    let mut man = manager();
    let prev_id = man.insert_new(control(), tcp());

    man.on_connection_failed(prev_id).unwrap();

    // No immediate reconnect.
    assert_matches!(cq_of(&mut man).as_slice(), []);

    time::advance(RECON_INTERVAL).await;

    // Reconnect after a while.
    let failed_id =
        assert_matches!(cq_of(&mut man).as_slice(), [(StateInner::Failed { .. }, id)] => *id);

    // ID should not be reused.
    assert_ne!(prev_id, failed_id);
}
