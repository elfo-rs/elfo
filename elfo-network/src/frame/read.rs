use crate::{
    codec::{
        self,
        decode::{DecodeState, DecodeStats},
        format::NetworkEnvelope,
    },
    frame::{
        buffers::{ReadBuffer, COMPRESSED_DATA_BUFFER_CAPACITY, DECOMPRESSED_DATA_BUFFER_CAPACITY},
        lz4::{DecompressState, DecompressStats, LZ4Buffer},
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
    /// `buffer`.
    NeedMoreData { buffer: &'a mut [u8] },
    /// The strategy successfully decoded an envelope from the frame.
    Done { decoded: NetworkEnvelope },
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

pub(crate) struct LZ4FramedRead {
    compressed_buffer: ReadBuffer,
    decompressed_buffer: LZ4Buffer,
    stats: FramedReadStats,
    position: usize,
}

impl LZ4FramedRead {
    pub(crate) fn new() -> Self {
        Self {
            compressed_buffer: ReadBuffer::with_capacity(COMPRESSED_DATA_BUFFER_CAPACITY),
            decompressed_buffer: LZ4Buffer::with_capacity(DECOMPRESSED_DATA_BUFFER_CAPACITY),
            stats: Default::default(),
            position: 0,
        }
    }
}

impl FramedReadStrategy for LZ4FramedRead {
    fn read(&mut self) -> Result<FramedReadState<'_>> {
        'decompression: loop {
            // We have finished decoding the current frame, try decompressing the next one.
            if self.position == self.decompressed_buffer.get_ref().len() {
                self.position = 0;

                let lz4_state = self.decompressed_buffer.decompress_frame(
                    self.compressed_buffer.filled_slice(),
                    &mut self.stats.decompress_stats,
                )?;
                match lz4_state {
                    DecompressState::NeedMoreData {
                        total_length_estimate,
                    } => {
                        // Decompression should not ask for less data than there is in the buffer.
                        debug_assert!(total_length_estimate > self.compressed_buffer.filled_len());

                        // NOTE: Here we rely on the fact that the decompressed was cleared, so that
                        // the next time `read()` method is called, we will go into this branch
                        // again.
                        return Ok(FramedReadState::NeedMoreData {
                            buffer: self
                                .compressed_buffer
                                .extend_to_contain(total_length_estimate),
                        });
                    }
                    DecompressState::Done { compressed_size } => {
                        // The new frame was decompressed, continue to decoding
                        // below.
                        self.compressed_buffer.consume_filled(compressed_size);
                    }
                }
            }

            // Try decoding messages from the decompressed frame. It is possible for all
            // messages to be invalid in the frame. In this case, all of them
            // will be skipped and we will try to decompress the next frame.
            'decoding: loop {
                let envelope_buffer = &self.decompressed_buffer.get_ref()[self.position..];
                let codec_state =
                    codec::decode::decode(envelope_buffer, &mut self.stats.decode_stats)?;
                match codec_state {
                    DecodeState::NeedMoreData { .. } => {
                        if self.position == self.decompressed_buffer.get_ref().len() {
                            // All messages in the frame were invalid, try
                            // decompressing the next one.
                            continue 'decompression;
                        } else {
                            // The frame must contain full messages.
                            return Err(eyre!(
                                "lz4 decompressed data contains truncated envelopes"
                            ));
                        }
                    }
                    DecodeState::Skipped { bytes_consumed } => {
                        self.position += bytes_consumed;
                        continue 'decoding;
                    }
                    DecodeState::Done {
                        bytes_consumed,
                        decoded,
                    } => {
                        self.position += bytes_consumed;
                        return Ok(FramedReadState::Done { decoded });
                    }
                }
            }
        }
    }

    fn mark_filled(&mut self, count: usize) {
        self.compressed_buffer.extend_filled(count);
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
        loop {
            let codec_state =
                codec::decode::decode(self.buffer.filled_slice(), &mut self.stats.decode_stats)?;
            match codec_state {
                DecodeState::NeedMoreData {
                    total_length_estimate,
                } => {
                    // Decoder should not ask for less data than there is in the buffer.
                    debug_assert!(total_length_estimate > self.buffer.filled_len());

                    break Ok(FramedReadState::NeedMoreData {
                        buffer: self.buffer.extend_to_contain(total_length_estimate),
                    });
                }
                DecodeState::Skipped { bytes_consumed } => {
                    self.buffer.consume_filled(bytes_consumed);
                    continue;
                }
                DecodeState::Done {
                    bytes_consumed,
                    decoded,
                } => {
                    self.stats.decompress_stats.total_uncompressed_bytes += bytes_consumed as u64;
                    self.buffer.consume_filled(bytes_consumed);
                    break Ok(FramedReadState::Done { decoded });
                }
            }
        }
    }

    fn mark_filled(&mut self, count: usize) {
        self.buffer.extend_filled(count);
    }

    fn take_stats(&mut self) -> FramedReadStats {
        std::mem::take(&mut self.stats)
    }
}
