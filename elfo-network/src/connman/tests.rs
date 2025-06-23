use std::time::Duration;

use elfo_utils::time::{with_instant_mock, Instant};

use crate::{
    config::Transport,
    connman::{Config, ConnMan, ConnectTask, ConnectTransport, Task},
    protocol::ConnectionRole,
};

const RECON_INTERVAL: Duration = Duration::from_millis(100);

fn manager() -> ConnMan {
    ConnMan::new(Config {
        reconnect_interval: RECON_INTERVAL,
    })
}

fn tcp() -> ConnectTransport {
    ConnectTransport::remote(Transport::Tcp("0.0.0.0:1337".to_owned()))
}

fn drain_tasks(man: &mut ConnMan) -> Vec<Task> {
    man.drain_task_queue().collect()
}

#[test]
fn tq_test() {
    with_instant_mock(|_| {
        let mut man = manager();

        // 1. Connection is inserted in establishing state.
        let conn = man.insert(ConnectionRole::Control, tcp());
        let id = conn.id();
        assert!(conn.state().is_establishing());

        // 2. Inserting new connection adds connect to the queue.
        let tasks = drain_tasks(&mut man);
        assert_eq!(
            tasks,
            vec![Task::Connect(ConnectTask {
                at: Instant::MIN,
                id,
            })]
        );

        man.remove(id);

        // 3. `Establishing -> Failed` transition should remove connect from the
        // task queue and insert connect after a specified period.
        let mut conn = man.insert(ConnectionRole::Control, tcp());
        let id = conn.id();
        conn.change_state(|t| t.failed());

        let tasks = drain_tasks(&mut man);
        assert_eq!(
            tasks,
            vec![Task::Connect(ConnectTask {
                at: Instant::now().checked_add(RECON_INTERVAL).unwrap(),
                id,
            })]
        );
    });
}
