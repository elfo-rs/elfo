use eyre::Result;

use elfo_core::{node::NodeNo, Context, GroupNo};

use crate::{actors::Key, config::Config};

pub(crate) struct Receiver {
    ctx: Context<Config, Key>,
    node_no: NodeNo,
    local_group_no: GroupNo,
}

impl Receiver {
    pub(super) fn new(ctx: Context<Config, Key>, node_no: NodeNo, local_group_no: GroupNo) -> Self {
        Self {
            ctx,
            node_no,
            local_group_no,
        }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // TODO
        Ok(())
    }
}
