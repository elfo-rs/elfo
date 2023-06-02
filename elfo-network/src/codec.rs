//! This module contains `Encoder` and `Decoder` instances
//! implementing the following frame structure of messages:
//! * size of rest frame, `u32`
//! * TODO: checksum
//! * protocol's length, `u8`
//! * protocol
//! * name's length, `u8`
//! * name
//! * trace_id, `u64`
//! * sender, `u64`
//! * target, `u64`
//! * kind, `u8`: 0 = Regular, 1 = RequestAny, 2 = RequestAll, 3 = Response
//! * correlation_id (for RequestAny, RequestAll and Response kinds), `u64`
//! * responses_left (for Response kind), `u32`
//! * message, rest of frame

use std::{convert::TryFrom, str};

use bytes::{Buf, BufMut, BytesMut};
use eyre::{ensure, eyre, Error, Result};
use tokio_util::codec;

use elfo_core::{
    tracing::TraceId,
    Addr, Envelope,
    _priv::{AnyMessage, MessageKind},
};

// === Encoder ===

const BUFFER_INITIAL_CAPACITY: usize = 4096;

pub(crate) struct Encoder {
    max_frame_size: u32,
    buffer: Vec<u8>, // TODO: use `UninitSlice` or a limited writer to avoid copying.
}

impl Encoder {
    pub(crate) fn new(max_frame_size: u32) -> Self {
        Self {
            max_frame_size,
            buffer: vec![0; BUFFER_INITIAL_CAPACITY],
        }
    }

    pub(crate) fn configure(&mut self, max_frame_size: u32) {
        self.max_frame_size = max_frame_size;
    }
}

impl codec::Encoder<(Envelope, Addr)> for Encoder {
    type Error = Error;

    fn encode(&mut self, (envelope, target): (Envelope, Addr), dst: &mut BytesMut) -> Result<()> {
        let message = envelope.message();
        let original_len = dst.len();

        // size
        dst.put_u32_le(0); // will be rewrite later.

        // protocol
        let protocol_len = message.protocol().len();
        assert!(protocol_len <= 255);
        dst.put_u8(protocol_len as u8);
        dst.extend_from_slice(message.protocol().as_bytes());

        // name
        let message_name_len = message.name().len();
        assert!(message_name_len <= 255);
        dst.put_u8(message_name_len as u8);
        dst.extend_from_slice(message.name().as_bytes());

        // trace_id
        dst.put_u64_le(u64::from(envelope.trace_id()));

        // sender
        let sender = envelope.sender().into_remote().into_bits();
        dst.put_u64_le(sender);

        // target
        dst.put_u64_le(target.into_bits());

        // kind
        // TODO: support requests and responses.
        dst.put_u8(0);

        // TODO: resize buffer on failures & limit size.
        let message_size = match message.write_msgpack(&mut self.buffer) {
            Ok(size) => size,
            Err(err) => {
                dst.truncate(original_len);
                return Err(err.into());
            }
        };

        // message
        dst.extend_from_slice(&self.buffer[..message_size]);

        let size = dst.len() - original_len - 4;
        (&mut dst[original_len..]).put_u32_le(size as u32);

        Ok(())
    }
}

// === Decoder ===

pub(crate) struct Decoder {
    max_frame_size: u32,
}

impl Decoder {
    pub(crate) fn new(max_frame_size: u32) -> Self {
        Self { max_frame_size }
    }

    pub(crate) fn configure(&mut self, max_frame_size: u32) {
        self.max_frame_size = max_frame_size;
    }
}

impl codec::Decoder for Decoder {
    type Error = Error;
    type Item = (Envelope, Addr);

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if src.len() < 4 {
            return Ok(None);
        }

        let mut buffer = &src[..];
        let size = buffer.get_u32_le();

        ensure!(
            size <= self.max_frame_size,
            "frame of length {} is too large, max allowed length is {}",
            size,
            self.max_frame_size,
        );

        let size = size as usize;
        if buffer.len() < size {
            let additional = size - buffer.len();
            src.reserve(additional);
            return Ok(None);
        }

        let data = decode(&buffer[..size]);
        src.advance(size);

        if let Err(err) = &data {
            // TODO: cooldown.
            tracing::error!(message = "cannot decode message, skipping", error = %err);
        }

        Ok(data.ok())
    }
}

fn decode(mut frame: &[u8]) -> Result<(Envelope, Addr)> {
    let protocol = get_str(&mut frame)?;
    let name = get_str(&mut frame)?;

    let trace_id = TraceId::try_from(frame.get_u64_le())?;
    let sender = Addr::from_bits(frame.get_u64_le());
    let target = Addr::from_bits(frame.get_u64_le());

    // TODO: support requests and responses.
    let kind = frame.get_u8();
    assert_eq!(kind, 0);
    let kind = MessageKind::Regular { sender };

    let message = AnyMessage::read_msgpack(protocol, name, frame)?
        .ok_or_else(|| eyre!("unknown message {}::{}", protocol, name))?;

    Ok((Envelope::with_trace_id(message, kind, trace_id), target))
}

fn get_str<'a>(frame: &mut &'a [u8]) -> Result<&'a str> {
    let len = usize::from(frame.get_u8());
    // TODO: It's not enough, still can fail if `len` is wrong.
    ensure!(len < frame.len(), "invalid header");
    let name = str::from_utf8(&frame[..len])?;
    frame.advance(len);
    Ok(name)
}

#[cfg(test)]
mod tests {
    use tokio_util::codec::{Decoder as _, Encoder as _};

    use elfo_core::{assert_msg_eq, message};

    use super::*;

    #[test]
    fn it_works() {
        #[message]
        #[derive(PartialEq)]
        struct Test(u32);

        let test = Test(42);
        let sender = Addr::NULL;
        let target = Addr::NULL;

        let trace_id = TraceId::try_from(42).unwrap();
        let envelope = Envelope::with_trace_id(
            AnyMessage::new(test),
            MessageKind::Regular { sender },
            trace_id,
        );

        let mut encoder = Encoder::new(1024);
        let mut decoder = Decoder::new(1024);

        let mut bytes = BytesMut::new();
        encoder.encode((envelope, target), &mut bytes).unwrap();

        let (actual_envelope, actual_target) = decoder.decode(&mut bytes).unwrap().unwrap();

        assert_eq!(actual_envelope.trace_id(), trace_id);
        assert_eq!(actual_envelope.sender(), sender);
        assert_eq!(actual_target, target);
        assert_msg_eq!(actual_envelope, Test(42));
    }
}
