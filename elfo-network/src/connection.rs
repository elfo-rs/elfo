use eyre::Result;

use elfo_core::{node::NodeNo, GroupNo, Topology};

use crate::NetworkContext;

pub(crate) struct Connection {
    ctx: NetworkContext,
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
        Self { ctx, local, remote }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // TODO
        Ok(())
    }
}
