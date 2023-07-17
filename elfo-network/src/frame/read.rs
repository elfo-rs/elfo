use crate::{
    codec::{
        self,
        decode::{DecodeState, DecodeStats},
        format::NetworkEnvelope,
    },
    frame::{
        buffers::{ReadBuffer, COMPRESSED_DATA_BUFFER_CAPACITY, DECOMPRESSED_DATA_BUFFER_CAPACITY},
        lz4::{DecompressStats, LZ4Buffer},
    },
};

use derive_more::Display;
use eyre::{eyre, Result};

#[derive(Display)]
pub(crate) enum FramedRead {
    #[display(fmt = "LZ4")]
    LZ4(LZ4FramedRead),
    #[display(fmt = "None")]
    None(NoneFramedRead),
}

impl FramedRead {
    pub(crate) fn lz4() -> Self {
        FramedRead::LZ4(LZ4FramedRead::new())
    }

    pub(crate) fn none() -> Self {
        FramedRead::None(NoneFramedRead::new())
    }
}

pub(crate) enum FramedReadState<'a> {
    /// The stategy needs more data written at the beginning of the specified
    /// `buffer`. At least `min_bytes` bytes are required to make progress,
    /// but the capacity of `buffer` would be typically larger than that.
    NeedMoreData {
        buffer: &'a mut [u8],
        min_bytes: usize,
    },
    /// `Some(envelope)` means that the strategy successfully decoded and
    /// envelope from the frame. `None` means that the frame has ended.
    Done { decoded: Option<NetworkEnvelope> },
}

#[derive(Default)]
pub(crate) struct FramedReadStats {
    /// Stats for decompression.
    pub(crate) decompress_stats: DecompressStats,
    /// Stats for decoding, which always happens on uncompressed data.
    pub(crate) decode_stats: DecodeStats,
}

pub(crate) trait FramedReadStrategy {
    fn read(&mut self) -> Result<FramedReadState<'_>>;

    fn mark_filled(&mut self, count: usize);

    fn take_stats(&mut self) -> FramedReadStats;
}

/// Hand-rolled dynamic dispatch to use branch predictor and allow
/// optimizations.
impl FramedReadStrategy for FramedRead {
    fn read(&mut self) -> Result<FramedReadState<'_>> {
        match self {
            FramedRead::LZ4(lz4) => lz4.read(),
            FramedRead::None(none) => none.read(),
        }
    }

    fn mark_filled(&mut self, count: usize) {
        match self {
            FramedRead::LZ4(lz4) => lz4.mark_filled(count),
            FramedRead::None(none) => none.mark_filled(count),
        }
    }

    fn take_stats(&mut self) -> FramedReadStats {
        match self {
            FramedRead::LZ4(lz4) => lz4.take_stats(),
            FramedRead::None(none) => none.take_stats(),
        }
    }
}

enum LZ4DecodingState {
    FrameDecompression,
    MessageParsing {
        compressed_size: usize,
        position: usize,
    },
}

pub(crate) struct LZ4FramedRead {
    compressed_buffer: ReadBuffer,
    decompressed_buffer: LZ4Buffer,
    state: LZ4DecodingState,
    stats: FramedReadStats,
}

impl LZ4FramedRead {
    pub(crate) fn new() -> Self {
        Self {
            compressed_buffer: ReadBuffer::with_capacity(COMPRESSED_DATA_BUFFER_CAPACITY),
            decompressed_buffer: LZ4Buffer::with_capacity(DECOMPRESSED_DATA_BUFFER_CAPACITY),
            state: LZ4DecodingState::FrameDecompression,
            stats: Default::default(),
        }
    }
}

impl FramedReadStrategy for LZ4FramedRead {
    fn read(&mut self) -> Result<FramedReadState<'_>> {
        let (compressed_size, position) = match self.state {
            LZ4DecodingState::FrameDecompression => {
                let lz4_state = self.decompressed_buffer.decompress_frame(
                    self.compressed_buffer.as_slice(),
                    &mut self.stats.decompress_stats,
                )?;
                match lz4_state {
                    DecodeState::NeedMoreData {
                        total_length_estimate,
                    } => {
                        self.compressed_buffer
                            .extend_to_contain(total_length_estimate);
                        let min_bytes = total_length_estimate - self.compressed_buffer.len();
                        return Ok(FramedReadState::NeedMoreData {
                            buffer: self.compressed_buffer.remaining(),
                            min_bytes,
                        });
                    }
                    DecodeState::Done { bytes_consumed, .. } => {
                        self.state = LZ4DecodingState::MessageParsing {
                            compressed_size: bytes_consumed,
                            position: 0,
                        };
                        (bytes_consumed, 0)
                    }
                }
            }
            LZ4DecodingState::MessageParsing {
                compressed_size,
                position,
            } => (compressed_size, position),
        };

        let decompressed_buffer = self.decompressed_buffer.get_ref();
        if position >= decompressed_buffer.len() {
            self.compressed_buffer.consume(compressed_size);
            self.state = LZ4DecodingState::FrameDecompression;
            return Ok(FramedReadState::Done { decoded: None });
        }

        let envelope_buffer = &decompressed_buffer[position..];
        let codec_state = codec::decode::decode(envelope_buffer, &mut self.stats.decode_stats)?;
        match codec_state {
            DecodeState::NeedMoreData { .. } => {
                Err(eyre!("lz4 decompressed data contains truncated envelopes"))
            }
            DecodeState::Done {
                bytes_consumed,
                decoded,
            } => {
                self.state = LZ4DecodingState::MessageParsing {
                    compressed_size,
                    position: position + bytes_consumed,
                };
                Ok(FramedReadState::Done {
                    decoded: Some(decoded),
                })
            }
        }
    }

    fn mark_filled(&mut self, count: usize) {
        self.compressed_buffer.mark_filled(count);
    }

    fn take_stats(&mut self) -> FramedReadStats {
        std::mem::take(&mut self.stats)
    }
}

pub(crate) struct NoneFramedRead {
    buffer: ReadBuffer,
    stats: FramedReadStats,
}

impl NoneFramedRead {
    pub(crate) fn new() -> Self {
        Self {
            buffer: ReadBuffer::with_capacity(DECOMPRESSED_DATA_BUFFER_CAPACITY),
            stats: Default::default(),
        }
    }
}

impl FramedReadStrategy for NoneFramedRead {
    fn read(&mut self) -> Result<FramedReadState<'_>> {
        let codec_state =
            codec::decode::decode(self.buffer.as_slice(), &mut self.stats.decode_stats)?;
        match codec_state {
            DecodeState::NeedMoreData {
                total_length_estimate,
            } => {
                self.buffer.extend_to_contain(total_length_estimate);
                let min_bytes = total_length_estimate - self.buffer.len();
                Ok(FramedReadState::NeedMoreData {
                    buffer: self.buffer.remaining(),
                    min_bytes,
                })
            }
            DecodeState::Done {
                bytes_consumed,
                decoded,
            } => {
                self.stats.decompress_stats.total_uncompressed_bytes += bytes_consumed as u64;
                self.buffer.consume(bytes_consumed);
                Ok(FramedReadState::Done {
                    decoded: Some(decoded),
                })
            }
        }
    }

    fn mark_filled(&mut self, count: usize) {
        self.buffer.mark_filled(count);
    }

    fn take_stats(&mut self) -> FramedReadStats {
        std::mem::take(&mut self.stats)
    }
}
