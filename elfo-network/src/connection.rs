use eyre::Result;

use elfo_core::{node::NodeNo, GroupNo};

use crate::NetworkContext;

pub(crate) struct Connection {
    ctx: NetworkContext,
    local: GroupNo,
    remote: (NodeNo, GroupNo),
}

impl Connection {
    pub(super) fn new(ctx: NetworkContext, local: GroupNo, remote: (NodeNo, GroupNo)) -> Self {
        Self { ctx, local, remote }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // TODO
        Ok(())
    }
}
