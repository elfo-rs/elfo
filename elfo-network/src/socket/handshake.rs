use std::io::Cursor;

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use eyre::{eyre, Result};
use tokio::io;

use elfo_core::addr::{NodeLaunchId, NodeNo};

use super::{raw, Capabilities};

const THIS_NODE_VERSION: u8 = 0;

pub(super) struct Handshake {
    pub(super) version: u8,
    pub(super) node_no: NodeNo,
    pub(super) launch_id: NodeLaunchId,
    pub(super) capabilities: Capabilities,
}

// NOTE: 16 bytes at the end are reserved.
const HANDSHAKE_LENGTH: usize = 39;
const HANDSHAKE_MAGIC: u64 = 0xE1F0E1F0E1F0E1F0;

impl Handshake {
    pub(crate) fn make_containing_buf() -> Vec<u8> {
        vec![0; HANDSHAKE_LENGTH]
    }

    pub(super) fn new(
        node_no: NodeNo,
        launch_id: NodeLaunchId,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            version: THIS_NODE_VERSION,
            node_no,
            launch_id,
            capabilities,
        }
    }

    pub(super) fn as_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Cursor::new(Self::make_containing_buf());

        buf.write_u64::<LittleEndian>(HANDSHAKE_MAGIC)?;
        buf.write_u8(self.version)?;
        buf.write_u16::<LittleEndian>(self.node_no.into_bits())?;
        buf.write_u64::<LittleEndian>(self.launch_id.into_bits())?;
        buf.write_u32::<LittleEndian>(self.capabilities.bits())?;

        let result = buf.into_inner();
        debug_assert_eq!(result.len(), HANDSHAKE_LENGTH);
        Ok(result)
    }

    pub(super) fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HANDSHAKE_LENGTH {
            return Err(eyre!(
                "expected handshake of length {}, got {} instead",
                HANDSHAKE_LENGTH,
                bytes.len()
            ));
        }

        let mut input = Cursor::new(bytes);

        if input.read_u64::<LittleEndian>()? != HANDSHAKE_MAGIC {
            return Err(eyre!("handshake magic did not match"));
        }

        let result = Self {
            version: input.read_u8()?,
            node_no: NodeNo::from_bits(input.read_u16::<LittleEndian>()?)
                .ok_or_else(|| eyre!("invalid node no"))?,
            launch_id: NodeLaunchId::from_bits(input.read_u64::<LittleEndian>()?),
            capabilities: Capabilities::from_bits_truncate(input.read_u32::<LittleEndian>()?),
        };

        Ok(result)
    }
}

pub(super) async fn handshake(
    raw_socket: &mut raw::Socket,
    node_no: NodeNo,
    launch_id: NodeLaunchId,
    capabilities: Capabilities,
) -> Result<Handshake> {
    let this_node_handshake = Handshake::new(node_no, launch_id, capabilities);
    io::AsyncWriteExt::write_all(&mut raw_socket.write, &this_node_handshake.as_bytes()?).await?;

    let mut buffer = Handshake::make_containing_buf();
    io::AsyncReadExt::read_exact(&mut raw_socket.read, &mut buffer).await?;
    let other_node_handshake = Handshake::from_bytes(&buffer)?;

    let version = this_node_handshake
        .version
        .min(other_node_handshake.version);
    let capabilities = this_node_handshake
        .capabilities
        .intersection(other_node_handshake.capabilities);

    Ok(Handshake {
        version,
        node_no: other_node_handshake.node_no,
        launch_id: other_node_handshake.launch_id,
        capabilities,
    })
}
