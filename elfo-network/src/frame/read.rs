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
    position: usize,
    filled: usize,
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
            position: 0,
            filled: 0,
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
                    .decompress_frame(&self.compressed_buffer[self.position..self.filled])?;
                match lz4_state {
                    DecodeState::NeedMoreData { length_estimate } => {
                        // Decoder requested for more data to be read. We round up the number of
                        // requested bytes to be a multiple of the chunk size to avoid
                        // reading too few bytes per syscall.
                        debug_assert!(length_estimate > self.filled - self.position);
                        let rounded_length = ((length_estimate + BUFFER_CHUNK_SIZE - 1)
                            / BUFFER_CHUNK_SIZE)
                            * BUFFER_CHUNK_SIZE;

                        let postfix_free_space = self.compressed_buffer.len() - self.filled;
                        if rounded_length <= postfix_free_space {
                            // There is enough space in the buffer for the
                            // requested data to be read
                            // into.
                        } else {
                            // Shift the data into the beginning of the buffer.
                            unsafe {
                                let buffer_start = self.compressed_buffer.as_mut_ptr();
                                let data_start = buffer_start.add(self.position);
                                let len = self.filled - self.position;
                                std::ptr::copy(data_start, buffer_start, len);

                                self.position = 0;
                                self.filled = len;
                            }

                            let total_free_space = self.compressed_buffer.len() - self.filled;
                            if rounded_length > total_free_space {
                                // If there is still not enough space after the shift, we need to
                                // reallocate.
                                let new_buffer_size = self.filled + rounded_length;
                                self.compressed_buffer.resize(new_buffer_size, 0);
                            }
                        }

                        return Ok(FramedReadState::NeedMoreData {
                            buffer: &mut self.compressed_buffer[self.filled..],
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
            self.position = self.filled;
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
        debug_assert!(self.filled + count < self.compressed_buffer.len());
        self.filled += count;
    }

    fn take_stats(&mut self) -> DecoderDeltaStats {
        std::mem::take(&mut self.stats)
    }
}
