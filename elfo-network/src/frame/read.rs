use crate::{
    codec::{DecoderDeltaStats, NetworkEnvelope},
    codec_direct::{self, DecodeState},
    frame::lz4::LZ4Buffer,
};

use eyre::{eyre, Result};

const BUFFER_INITIAL_CAPACITY: usize = 8192;

pub(crate) enum FramedRead {
    LZ4(LZ4FramedRead),
}

impl FramedRead {
    pub(crate) fn lz4() -> Self {
        FramedRead::LZ4(LZ4FramedRead::new())
    }
}

pub(crate) trait FramedReadStrategy {
    fn read(&mut self, input: &[u8]) -> Result<DecodeState<Option<NetworkEnvelope>>>;

    fn take_stats(&mut self) -> DecoderDeltaStats;
}

/// Hand-rolled dynamic dispatch to use branch predictor and allow
/// optimizations.
impl FramedReadStrategy for FramedRead {
    fn read(&mut self, input: &[u8]) -> Result<DecodeState<Option<NetworkEnvelope>>> {
        match self {
            FramedRead::LZ4(lz4) => lz4.read(input),
        }
    }

    fn take_stats(&mut self) -> DecoderDeltaStats {
        match self {
            FramedRead::LZ4(lz4) => lz4.take_stats(),
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
    buffer: LZ4Buffer,
    state: LZ4DecodingState,
    stats: DecoderDeltaStats,
}

impl LZ4FramedRead {
    pub(crate) fn new() -> Self {
        Self {
            buffer: LZ4Buffer::with_capacity(BUFFER_INITIAL_CAPACITY),
            state: LZ4DecodingState::FrameDecompression,
            stats: Default::default(),
        }
    }
}

impl FramedReadStrategy for LZ4FramedRead {
    fn read(&mut self, input: &[u8]) -> Result<DecodeState<Option<NetworkEnvelope>>> {
        let (compressed_size, position) = match self.state {
            LZ4DecodingState::FrameDecompression => {
                let lz4_state = self.buffer.decompress_frame(input)?;
                match lz4_state {
                    DecodeState::NeedMoreData { length_estimate } => {
                        return Ok(DecodeState::NeedMoreData { length_estimate })
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

        let uncompressed_buffer = self.buffer.get_ref();
        if position >= uncompressed_buffer.len() {
            self.state = LZ4DecodingState::FrameDecompression;
            return Ok(DecodeState::Done {
                bytes_consumed: compressed_size,
                decoded: None,
            });
        }

        let envelope_buffer = &uncompressed_buffer[position..];
        let codec_state = codec_direct::decode(envelope_buffer, &mut self.stats)?;
        match codec_state {
            DecodeState::NeedMoreData { .. } => {
                return Err(eyre!("lz4 decompressed data contains truncated envelopes"))
            }
            DecodeState::Done {
                bytes_consumed,
                decoded,
            } => {
                self.state = LZ4DecodingState::MessageParsing {
                    compressed_size,
                    position: position + bytes_consumed,
                };
                // We do not want the outer code to move the input buffer yet, so we return as
                // if we consumed zero bytes.
                return Ok(DecodeState::Done {
                    bytes_consumed: 0,
                    decoded: Some(decoded),
                });
            }
        }
    }

    fn take_stats(&mut self) -> DecoderDeltaStats {
        std::mem::take(&mut self.stats)
    }
}
