use crate::{
    codec::{EncodeError, EncoderDeltaStats, NetworkEnvelope},
    codec_direct,
    lz4::LZ4Buffer,
};

use eyre::Result;

pub(crate) trait FramedWrite {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<(), EncodeError>;

    fn prepare_next_frame(&mut self);

    fn finalize(&mut self) -> Result<&[u8]>;

    fn take_stats(&mut self) -> EncoderDeltaStats;
}

pub(crate) struct LZ4FramedWrite {
    uncompressed_buffer: Vec<u8>,
    compressed_buffer: LZ4Buffer,
    stats: EncoderDeltaStats,
    envelope_size_limit: Option<usize>,
}

impl LZ4FramedWrite {
    pub(crate) fn new() -> Self {
        // TODO: pass limits and set initial sizes.
        Self {
            uncompressed_buffer: Vec::new(),
            compressed_buffer: LZ4Buffer::new(),
            stats: Default::default(),
            envelope_size_limit: None,
        }
    }
}

impl FramedWrite for LZ4FramedWrite {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<(), EncodeError> {
        codec_direct::encode(
            envelope,
            &mut self.uncompressed_buffer,
            &mut self.stats,
            self.envelope_size_limit,
        )
    }

    fn prepare_next_frame(&mut self) {
        self.uncompressed_buffer.clear();
        self.compressed_buffer.reset();
    }

    fn finalize(&mut self) -> Result<&[u8]> {
        self.compressed_buffer
            .compress_frame(&self.uncompressed_buffer)
    }

    fn take_stats(&mut self) -> EncoderDeltaStats {
        std::mem::take(&mut self.stats)
    }
}
