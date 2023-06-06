use eyre::Result;

use elfo_core::{
    errors::{SendError, TrySendError},
    messages::Impossible,
    msg,
    node::NodeNo,
    stream::Stream,
    Addr, Envelope, GroupNo, Topology, UnattachedSource,
};

use crate::{protocol::HandleConnection, socket::WriteHalf, NetworkContext};

pub(crate) struct Connection {
    ctx: NetworkContext,
    topology: Topology,
    local: (GroupNo, String),
    remote: (NodeNo, GroupNo, String),
}

impl Connection {
    pub(super) fn new(
        ctx: NetworkContext,
        local: (GroupNo, String),
        remote: (NodeNo, GroupNo, String),
        topology: Topology,
    ) -> Self {
        Self {
            ctx,
            topology,
            local,
            remote,
        }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // Receive the socket. It must always be a first message.
        let socket = msg!(match self.ctx.try_recv().await? {
            HandleConnection { socket, .. } => socket.take().unwrap(),
            _ => unreachable!("unexpected initial message"),
        });

        // Register `RemoteHandle`. Now we can receive messages from local groups.
        let (local_tx, local_rx) = kanal::unbounded_async();
        let remote_handle = RemoteHandle { tx: local_tx };
        let _guard = self.topology.register_remote(
            self.local.0,
            (self.remote.0, self.remote.1),
            &self.remote.2,
            remote_handle,
        );

        // Start handling local incoming messages.
        self.ctx
            .attach(make_local_rx_handler(local_rx, socket.write));

        while let Some(_envelope) = self.ctx.recv().await {
            // TODO: graceful termination
            // TODO: handle another `HandleConnection`
        }

        Ok(())
    }
}

fn make_local_rx_handler(
    rx: kanal::AsyncReceiver<(Addr, Envelope)>,
    mut tx: WriteHalf,
) -> UnattachedSource<Stream<Impossible>> {
    // We should write messages as many as possible at once to have better
    // compression rate and reduce the number of system calls.
    // On the other hand, we should minimize the time which every message is unsent.
    // Thus, we should find a balance between these two factors, some trade-off.
    // The current strategy is to send all available messages and then forcibly
    // flush intermediate buffers to the socket.
    //
    // Also, tokio implements budget on sockets, so this subtask sometimes returns
    // the execution back to the runtime even in case of a full incoming queue.
    Stream::once(async move {
        loop {
            // TODO: trace_id, error handling.
            let (mut recipient, mut envelope) = rx.recv().await.unwrap();
            loop {
                // This call can actually write to the socket if the buffer is full.
                tx.send(envelope, recipient).await.unwrap();
                (recipient, envelope) = ward!(rx.try_recv().unwrap(), break);
            }

            // Forcibly write to the socket remaining data in the buffer, because
            // we don't know how long we'll wait for the next message.
            tx.flush().await.unwrap();
        }
    })
}

// === RemoteHandle ===

struct RemoteHandle {
    tx: kanal::AsyncSender<(Addr, Envelope)>,
}

impl elfo_core::remote::RemoteHandle for RemoteHandle {
    fn try_send(&self, recipient: Addr, envelope: Envelope) -> Result<(), TrySendError<Envelope>> {
        // TODO: semaphore.

        let mut item = Some((recipient, envelope));
        match self.tx.try_send_option(&mut item) {
            Ok(true) => Ok(()),
            Ok(false) => unreachable!(),
            Err(_) => Err(TrySendError::Closed(item.take().unwrap().1)),
        }
    }

    fn unbounded_send(
        &self,
        recipient: Addr,
        envelope: Envelope,
    ) -> Result<(), SendError<Envelope>> {
        let mut item = Some((recipient, envelope));
        match self.tx.try_send_option(&mut item) {
            Ok(true) => Ok(()),
            Ok(false) => unreachable!(),
            Err(_) => Err(SendError(item.take().unwrap().1)),
        }
    }
}
