use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;
use tracing::error;

use eyre::{eyre, Result};

use crate::codec_direct::DecodeState;

pub(crate) struct LZ4Buffer {
    buffer: Vec<u8>,
}

// TODO: checksums.
// TODO: proper framing. Currently the whole encoding is:
// 1. Size of the whole frame
// 2. Size of the uncompressed data
// 3. LZ4 compressed data

impl LZ4Buffer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.buffer.clear();
    }

    pub(crate) fn decompress_frame(&mut self, raw: &[u8]) -> Result<DecodeState<&[u8]>> {
        if raw.len() < 4 {
            return Ok(DecodeState::NeedMoreData { length_estimate: 4 });
        }

        let mut input = Cursor::new(raw);
        let frame_size = input.read_u32::<LittleEndian>()? as usize;
        if frame_size >= 200_000_000 {
            return Err(eyre!("frame size is too big"));
        }

        if raw.len() < frame_size {
            return Ok(DecodeState::NeedMoreData {
                length_estimate: frame_size,
            });
        }

        let decompressed_size = input.read_u32::<LittleEndian>()? as usize;
        if decompressed_size >= 200_000_000 {
            return Err(eyre!("uncompressed size is too big"));
        }

        self.buffer.clear();
        self.buffer.resize(decompressed_size, 0);

        // TODO: replace with `Cursor::remaining_slice` once it becomes stable.
        error!(message = "laplab: decompressing frame", frame_size = %frame_size, data_size = %decompressed_size);
        let remaining_slice = &input.get_ref()[input.position() as usize..frame_size];
        let actual_size = lz4_flex::block::decompress_into(remaining_slice, &mut self.buffer)?;
        if actual_size != decompressed_size {
            return Err(eyre!(
                "expected to decompress {} bytes, got {}",
                decompressed_size,
                actual_size
            ));
        }

        Ok(DecodeState::Done {
            bytes_consumed: frame_size,
            decoded: &self.buffer,
        })
    }

    pub(crate) fn get_ref(&self) -> &[u8] {
        &self.buffer
    }

    pub(crate) fn compress_frame(&mut self, input: &[u8]) -> Result<&[u8]> {
        self.buffer.clear();
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

        error!(message = "laplab: compressed frame", %frame_size, data_size = (input.len() as u32));

        Ok(&self.buffer)
    }
}
