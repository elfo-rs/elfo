use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

use eyre::{eyre, Result};

pub(crate) struct LZ4Buffer {
    buffer: Vec<u8>,
}

impl LZ4Buffer {
    pub(crate) fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub(crate) fn reset(&mut self) {
        self.buffer.clear();
    }

    pub(crate) fn decompress_frame(&mut self, input: &[u8]) -> Result<&[u8]> {
        self.buffer.clear();

        let mut input = Cursor::new(input);
        let decompressed_size = input.read_u32::<LittleEndian>()? as usize;
        if decompressed_size >= 200_000_000 {
            return Err(eyre!("uncompressed size is too big"));
        }
        self.buffer.resize(decompressed_size, 0);

        // TODO: replace with `Cursor::remaining_slice` once it becomes stable.
        let remaining_slice = &input.get_ref()[input.position() as usize..];
        let actual_size = lz4_flex::block::decompress_into(remaining_slice, &mut self.buffer)?;
        if actual_size != decompressed_size {
            return Err(eyre!(
                "expected to decompress {} bytes, got {}",
                decompressed_size,
                actual_size
            ));
        }

        Ok(&self.buffer)
    }

    pub(crate) fn compress_frame(&mut self, input: &[u8]) -> Result<&[u8]> {
        self.buffer.clear();
        self.buffer
            .resize(4 + lz4_flex::block::get_maximum_output_size(input.len()), 0);

        let mut output = Cursor::new(self.buffer.as_mut_slice());
        output.write_u32::<LittleEndian>(input.len() as u32)?;

        // TODO: replace with `Cursor::remaining_slice` once it becomes stable.
        let position = output.position() as usize;
        let remaining_slice = &mut output.get_mut()[position..];
        lz4_flex::block::compress_into(input, remaining_slice)?;

        Ok(&self.buffer)
    }
}
