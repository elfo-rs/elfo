//! This module implements custom LZ4 framing.
//!
//! Single LZ4 frame contains one or more envelopes compressed as a single
//! LZ4 block along with some meta information like frame size. We do NOT
//! use standard LZ4 framing.
//!
//! Structure of a single frame:
//!           name               bits
//! +---------------------------+----+
//! | size of whole frame       | 32 |
//! +---------------------------+----+
//! | size of decompressed data | 32 |
//! +---------------------------+----+
//! | LZ4 block                 |rest|
//! +---------------------------+----+
//!
//! All fields are encoded using LE ordering.

// TODO: checksums.

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

use eyre::{eyre, Result};

pub(crate) struct LZ4Buffer {
    /// This buffer stores decompressed data after `decompress_frame` method
    /// is called and compressed data after `compress_frame` method is called.
    buffer: Vec<u8>,
    len: usize,
}

#[derive(Default)]
pub(crate) struct DecompressStats {
    /// How many compressed bytes were processed so far.
    pub(crate) total_compressed_bytes: u64,
    /// How many uncompressed bytes were produced during decompression so far.
    pub(crate) total_uncompressed_bytes: u64,
}

#[derive(Default)]
pub(crate) struct CompressStats {
    /// How many uncompressed bytes were processed so far.
    pub(crate) total_uncompressed_bytes: u64,
    /// How many uncompressed bytes were produced during compression so far.
    pub(crate) total_compressed_bytes: u64,
}

pub(crate) enum DecompressState {
    /// Buffer needs to contain at least `total_length_estimate` bytes in total
    /// in order for the decompression algorithm to make progress.
    NeedMoreData { total_length_estimate: usize },
    /// A series of bytes was decompressed, which occupied `compressed_size`
    /// bytes when compressed.
    Done { compressed_size: usize },
}

// TODO: implement better system for limiting memory usage.
const MAX_FRAME_SIZE: usize = 200_000_000;

impl LZ4Buffer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: vec![0; capacity],
            len: 0,
        }
    }

    pub(crate) fn filled_slice(&self) -> &[u8] {
        &self.buffer[..self.len]
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn decompress_frame(
        &mut self,
        raw: &[u8],
        stats: &mut DecompressStats,
    ) -> Result<DecompressState> {
        self.len = 0;

        if raw.len() < 4 {
            return Ok(DecompressState::NeedMoreData {
                total_length_estimate: 4,
            });
        }

        let mut input = Cursor::new(raw);
        let frame_size = input.read_u32::<LittleEndian>()? as usize;
        if frame_size >= MAX_FRAME_SIZE {
            return Err(eyre!("frame size is too big"));
        }

        if raw.len() < frame_size {
            return Ok(DecompressState::NeedMoreData {
                total_length_estimate: frame_size,
            });
        }

        let decompressed_size = input.read_u32::<LittleEndian>()? as usize;
        if decompressed_size >= MAX_FRAME_SIZE {
            return Err(eyre!("decompressed size is too big"));
        }

        if decompressed_size > self.buffer.len() {
            self.buffer.resize(decompressed_size, 0);
        }

        // TODO: replace with `Cursor::remaining_slice` once it becomes stable.
        let remaining_slice = &input.get_ref()[input.position() as usize..frame_size];
        let actual_size = lz4_flex::block::decompress_into(remaining_slice, &mut self.buffer)?;
        if actual_size != decompressed_size {
            return Err(eyre!(
                "expected to decompress {} bytes, got {}",
                decompressed_size,
                actual_size
            ));
        }

        self.len = decompressed_size;

        stats.total_compressed_bytes += frame_size as u64;
        stats.total_uncompressed_bytes += decompressed_size as u64;

        Ok(DecompressState::Done {
            compressed_size: frame_size,
        })
    }

    pub(crate) fn compress_frame(
        &mut self,
        input: &[u8],
        stats: &mut CompressStats,
    ) -> Result<&[u8]> {
        self.buffer
            .resize(8 + lz4_flex::block::get_maximum_output_size(input.len()), 0);

        let mut output = Cursor::new(self.buffer.as_mut_slice());
        output.write_u32::<LittleEndian>(0)?; // Overwritten below.
        output.write_u32::<LittleEndian>(input.len() as u32)?;

        // TODO: replace with `Cursor::remaining_slice` once it becomes stable.
        let position = output.position() as usize;
        let remaining_slice = &mut output.get_mut()[position..];
        let compressed_size = lz4_flex::block::compress_into(input, remaining_slice)?;

        let frame_size = compressed_size + 8;
        output.set_position(0);
        output.write_u32::<LittleEndian>(frame_size as u32)?;

        self.buffer.resize(frame_size, 0);

        stats.total_uncompressed_bytes += input.len() as u64;
        stats.total_compressed_bytes += frame_size as u64;

        Ok(&self.buffer)
    }
}
