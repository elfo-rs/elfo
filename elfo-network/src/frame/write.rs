use crate::{
    codec::{
        self,
        encode::{EncodeError, EncoderDeltaStats},
        format::NetworkEnvelope,
    },
    frame::lz4::LZ4Buffer,
};

use eyre::Result;

const BUFFER_INITIAL_CAPACITY: usize = 8192;

#[derive(PartialEq, Eq)]
pub(crate) enum FrameState {
    // Accumulating,
    FlushAdvised,
}

pub(crate) enum FramedWrite {
    LZ4(LZ4FramedWrite),
}

impl FramedWrite {
    pub(crate) fn lz4(envelope_size_limit: Option<usize>) -> Self {
        FramedWrite::LZ4(LZ4FramedWrite::new(envelope_size_limit))
    }
}

pub(crate) trait FramedWriteStrategy {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<FrameState, EncodeError>;

    fn finalize(&mut self) -> Result<&[u8]>;

    fn take_stats(&mut self) -> EncoderDeltaStats;
}

/// Hand-rolled dynamic dispatch to use branch predictor and allow
/// optimizations.
impl FramedWriteStrategy for FramedWrite {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<FrameState, EncodeError> {
        match self {
            FramedWrite::LZ4(lz4) => lz4.write(envelope),
        }
    }

    fn finalize(&mut self) -> Result<&[u8]> {
        match self {
            FramedWrite::LZ4(lz4) => lz4.finalize(),
        }
    }

    fn take_stats(&mut self) -> EncoderDeltaStats {
        match self {
            FramedWrite::LZ4(lz4) => lz4.take_stats(),
        }
    }
}

pub(crate) struct LZ4FramedWrite {
    uncompressed_buffer: Vec<u8>,
    compressed_buffer: LZ4Buffer,
    stats: EncoderDeltaStats,
    envelope_size_limit: Option<usize>,
}

impl LZ4FramedWrite {
    pub(crate) fn new(envelope_size_limit: Option<usize>) -> Self {
        Self {
            uncompressed_buffer: Vec::with_capacity(BUFFER_INITIAL_CAPACITY),
            compressed_buffer: LZ4Buffer::with_capacity(BUFFER_INITIAL_CAPACITY),
            stats: Default::default(),
            envelope_size_limit,
        }
    }
}

impl FramedWriteStrategy for LZ4FramedWrite {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<FrameState, EncodeError> {
        codec::encode::encode(
            envelope,
            &mut self.uncompressed_buffer,
            &mut self.stats,
            self.envelope_size_limit,
        )?;
        // TODO: allow multiple envelopes in the frame.
        Ok(FrameState::FlushAdvised)
    }

    fn finalize(&mut self) -> Result<&[u8]> {
        self.compressed_buffer
            .compress_frame(&self.uncompressed_buffer)
    }

    fn take_stats(&mut self) -> EncoderDeltaStats {
        std::mem::take(&mut self.stats)
    }
}
