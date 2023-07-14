use crate::{
    codec::{
        self,
        decode::{DecodeState, DecoderDeltaStats},
        format::NetworkEnvelope,
    },
    frame::lz4::LZ4Buffer,
};

use eyre::{eyre, Result};

pub(crate) enum FramedRead {
    LZ4(LZ4FramedRead),
}

impl FramedRead {
    pub(crate) fn lz4() -> Self {
        FramedRead::LZ4(LZ4FramedRead::new())
    }
}

pub(crate) enum FramedReadState<'a> {
    NeedMoreData {
        buffer: &'a mut [u8],
        min_bytes: usize,
    },
    Done {
        decoded: Option<NetworkEnvelope>,
    },
}

pub(crate) trait FramedReadStrategy {
    fn read(&mut self) -> Result<FramedReadState<'_>>;

    fn advance(&mut self, count: usize);

    fn take_stats(&mut self) -> DecoderDeltaStats;
}

/// Hand-rolled dynamic dispatch to use branch predictor and allow
/// optimizations.
impl FramedReadStrategy for FramedRead {
    fn read(&mut self) -> Result<FramedReadState<'_>> {
        match self {
            FramedRead::LZ4(lz4) => lz4.read(),
        }
    }

    fn advance(&mut self, count: usize) {
        match self {
            FramedRead::LZ4(lz4) => lz4.advance(count),
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
    compressed_buffer: Vec<u8>,
    compressed_buffer_end: usize,
    decompressed_buffer: LZ4Buffer,
    state: LZ4DecodingState,
    stats: DecoderDeltaStats,
}

const COMPRESSED_DATA_BUFFER_CAPACITY: usize = 64 * 1024;
const DECOMPRESSED_DATA_BUFFER_CAPACITY: usize = 128 * 1024;

impl LZ4FramedRead {
    pub(crate) fn new() -> Self {
        Self {
            compressed_buffer: Vec::with_capacity(COMPRESSED_DATA_BUFFER_CAPACITY),
            compressed_buffer_end: 0,
            decompressed_buffer: LZ4Buffer::with_capacity(DECOMPRESSED_DATA_BUFFER_CAPACITY),
            state: LZ4DecodingState::FrameDecompression,
            stats: Default::default(),
        }
    }
}

const BUFFER_CHUNK_SIZE: usize = 512;

impl FramedReadStrategy for LZ4FramedRead {
    fn read(&mut self) -> Result<FramedReadState<'_>> {
        let (compressed_size, position) = match self.state {
            LZ4DecodingState::FrameDecompression => {
                let lz4_state = self
                    .decompressed_buffer
                    .decompress_frame(&self.compressed_buffer[..self.compressed_buffer_end])?;
                match lz4_state {
                    DecodeState::NeedMoreData { length_estimate } => {
                        // TODO: avoid zeroing memory.
                        // Decoder requested for more data to be read. We round up the number of
                        // requested bytes to be a multiple of the chunk size to avoid
                        // reading too few bytes per syscall.
                        debug_assert!(length_estimate > self.compressed_buffer_end);
                        let new_buffer_size = ((length_estimate + BUFFER_CHUNK_SIZE - 1)
                            / BUFFER_CHUNK_SIZE)
                            * BUFFER_CHUNK_SIZE;
                        self.compressed_buffer.resize(new_buffer_size, 0);
                        return Ok(FramedReadState::NeedMoreData {
                            buffer: &mut self.compressed_buffer[self.compressed_buffer_end..],
                            min_bytes: length_estimate,
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
            // We have finished decoding the current frame. In total, we have have processed
            // `compressed_size` bytes, but there still might be data left in the
            // `compressed_buffer` after that position. So we remove only the processed
            // data, keeping any tail left in the buffer untouched.
            // TODO: avoid zeroing memory.
            self.compressed_buffer.drain(..compressed_size);
            self.compressed_buffer_end -= compressed_size;
            self.state = LZ4DecodingState::FrameDecompression;
            return Ok(FramedReadState::Done { decoded: None });
        }

        let envelope_buffer = &decompressed_buffer[position..];
        let codec_state = codec::decode::decode(envelope_buffer, &mut self.stats)?;
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

    fn advance(&mut self, count: usize) {
        debug_assert!(self.compressed_buffer_end + count < self.compressed_buffer.len());
        self.compressed_buffer_end += count;
    }

    fn take_stats(&mut self) -> DecoderDeltaStats {
        std::mem::take(&mut self.stats)
    }
}
