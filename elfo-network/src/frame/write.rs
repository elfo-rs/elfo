use crate::{
    codec::{
        self,
        encode::{EncodeError, EncodeStats},
        format::NetworkEnvelope,
    },
    frame::lz4::{CompressStats, LZ4Buffer},
};

use eyre::Result;

use super::buffers::{COMPRESSED_DATA_BUFFER_CAPACITY, DECOMPRESSED_DATA_BUFFER_CAPACITY};

#[derive(PartialEq, Eq)]
pub(crate) enum FrameState {
    Accumulating,
    FlushAdvised,
}

pub(crate) enum FramedWrite {
    Lz4(LZ4FramedWrite),
    None(NoneFramedWrite),
}

impl FramedWrite {
    pub(crate) fn lz4(envelope_size_limit: Option<usize>) -> Self {
        FramedWrite::Lz4(LZ4FramedWrite::new(envelope_size_limit))
    }

    pub(crate) fn none(envelope_size_limit: Option<usize>) -> Self {
        FramedWrite::None(NoneFramedWrite::new(envelope_size_limit))
    }
}

#[derive(Default)]
pub(crate) struct FramedWriteStats {
    /// Stats for encoding, which always produces uncompressed data.
    pub(crate) encode_stats: EncodeStats,
    /// Stats for compression.
    pub(crate) compress_stats: CompressStats,
}

pub(crate) trait FramedWriteStrategy {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<FrameState, EncodeError>;

    fn finalize(&mut self) -> Result<&[u8]>;

    fn take_stats(&mut self) -> FramedWriteStats;
}

/// Hand-rolled dynamic dispatch to use branch predictor and allow
/// optimizations.
impl FramedWriteStrategy for FramedWrite {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<FrameState, EncodeError> {
        match self {
            FramedWrite::Lz4(lz4) => lz4.write(envelope),
            FramedWrite::None(none) => none.write(envelope),
        }
    }

    fn finalize(&mut self) -> Result<&[u8]> {
        match self {
            FramedWrite::Lz4(lz4) => lz4.finalize(),
            FramedWrite::None(none) => none.finalize(),
        }
    }

    fn take_stats(&mut self) -> FramedWriteStats {
        match self {
            FramedWrite::Lz4(lz4) => lz4.take_stats(),
            FramedWrite::None(none) => none.take_stats(),
        }
    }
}

pub(crate) struct LZ4FramedWrite {
    decompressed_buffer: Vec<u8>,
    compressed_buffer: LZ4Buffer,
    stats: FramedWriteStats,
    envelope_size_limit: Option<usize>,
}

impl LZ4FramedWrite {
    pub(crate) fn new(envelope_size_limit: Option<usize>) -> Self {
        Self {
            decompressed_buffer: Vec::with_capacity(DECOMPRESSED_DATA_BUFFER_CAPACITY),
            compressed_buffer: LZ4Buffer::with_capacity(COMPRESSED_DATA_BUFFER_CAPACITY),
            stats: Default::default(),
            envelope_size_limit,
        }
    }
}

/// How many bytes we aim at writing into the socket.
const OUTPUT_FLUSH_THRESHOLD: usize = 64 * 1024;

impl FramedWriteStrategy for LZ4FramedWrite {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<FrameState, EncodeError> {
        codec::encode::encode(
            envelope,
            &mut self.decompressed_buffer,
            &mut self.stats.encode_stats,
            self.envelope_size_limit,
        )?;

        // We conservatively estimate that LZ4 will provide us with x2 compression rate
        // on msgpack data.
        // TODO: improve estimate on actual compression rates.
        Ok(
            if self.decompressed_buffer.len() / 2 > OUTPUT_FLUSH_THRESHOLD {
                FrameState::FlushAdvised
            } else {
                FrameState::Accumulating
            },
        )
    }

    fn finalize(&mut self) -> Result<&[u8]> {
        let result = self
            .compressed_buffer
            .compress_frame(&self.decompressed_buffer, &mut self.stats.compress_stats);
        self.decompressed_buffer.clear();
        result?;
        Ok(self.compressed_buffer.filled_slice())
    }

    fn take_stats(&mut self) -> FramedWriteStats {
        std::mem::take(&mut self.stats)
    }
}

pub(crate) struct NoneFramedWrite {
    buffer: Vec<u8>,
    stats: FramedWriteStats,
    after_finalize: bool,
    envelope_size_limit: Option<usize>,
}

impl NoneFramedWrite {
    fn new(envelope_size_limit: Option<usize>) -> Self {
        Self {
            buffer: Vec::with_capacity(DECOMPRESSED_DATA_BUFFER_CAPACITY),
            stats: Default::default(),
            after_finalize: false,
            envelope_size_limit,
        }
    }
}

impl FramedWriteStrategy for NoneFramedWrite {
    fn write(&mut self, envelope: &NetworkEnvelope) -> Result<FrameState, EncodeError> {
        if self.after_finalize {
            self.buffer.clear();
            self.after_finalize = false;
        }

        codec::encode::encode(
            envelope,
            &mut self.buffer,
            &mut self.stats.encode_stats,
            self.envelope_size_limit,
        )?;

        Ok(if self.buffer.len() > OUTPUT_FLUSH_THRESHOLD {
            FrameState::FlushAdvised
        } else {
            FrameState::Accumulating
        })
    }

    fn finalize(&mut self) -> Result<&[u8]> {
        self.after_finalize = true;
        self.stats.compress_stats.total_uncompressed_bytes += self.buffer.len() as u64;
        Ok(&self.buffer)
    }

    fn take_stats(&mut self) -> FramedWriteStats {
        std::mem::take(&mut self.stats)
    }
}
