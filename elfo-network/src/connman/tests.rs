use std::time::Duration;

use elfo_utils::time::with_instant_mock;

use crate::{
    config::Transport,
    connman::{Config, Conn, ConnMan, ConnectTransport, State},
    protocol::ConnectionRole,
};

fn manager() -> ConnMan {
    ConnMan::new(Config {
        reconnect_interval: Duration::from_millis(100),
    })
}

fn tcp() -> ConnectTransport {
    ConnectTransport::remote(Transport::Tcp("0.0.0.0:1337".to_owned()))
}

#[test]
fn reconnection_works() {
    with_instant_mock(|_| {
        let mut man = manager();
        let mut conn = man.insert(Conn::new(ConnectionRole::Control, tcp()));
        let conn_id = conn.id();

        assert_eq!(conn.state(), State::Establishing);

        let advise = man
            .failed()
            .pop_for_retry()
            .expect_err("new connection must be in establishing state, thus not in queue");
        // And no advise should be given.
        assert_eq!(advise, None);

        // Nuh uh, borrowing.
        conn = man.get_mut(conn_id).unwrap();
        conn.change_state(|t| t.failed());

        let advise = man
            .failed()
            .pop_for_retry()
            .expect_err("manager must not be ready to reconnect at the time")
            .expect("manager must advise reconnection, since failed connection is just landed");
        // Must advise to reconnect after 100ms.
        assert_eq!(advise.after.as_millis(), 100);
        assert_eq!(advise.id, conn_id);

        let advise = man
            .failed()
            .pop_for_retry()
            .expect_err("queue must be empty now");
        assert_eq!(advise, None);
    });
}
