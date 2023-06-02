use eyre::Result;

use elfo_core::{node::NodeNo, Context, GroupNo};

use crate::{actors::Key, config::Config};

pub(crate) struct Transmitter {
    ctx: Context<Config, Key>,
    node_no: NodeNo,
    remote_group_no: GroupNo,
}

impl Transmitter {
    pub(super) fn new(
        ctx: Context<Config, Key>,
        node_no: NodeNo,
        remote_group_no: GroupNo,
    ) -> Self {
        Self {
            ctx,
            node_no,
            remote_group_no,
        }
    }

    pub(super) async fn main(mut self) -> Result<()> {
        // TODO
        Ok(())
    }
}
