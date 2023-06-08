//! This module contains `Encoder` and `Decoder` instances
//! implementing the following frame structure of messages:
//! * size of whole frame, `u32`
//! * TODO: checksum
//! * protocol's length, `u8`
//! * protocol
//! * name's length, `u8`
//! * name
//! * trace_id, `u64`
//! * sender, `u64`
//! * recipient, `u64`
//! * kind, `u8`: 0 = Regular, 1 = RequestAny, 2 = RequestAll, 3 = Response
//! * correlation_id (for RequestAny, RequestAll and Response kinds), `u64`
//! * message, rest of frame

use std::{convert::TryFrom, str};

use bytes::{Buf, BufMut, BytesMut};
use eyre::{bail, ensure, eyre, Error, Result};
use tokio_util::codec;

use elfo_core::{tracing::TraceId, Addr, Message, _priv::AnyMessage};

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

impl codec::Encoder<NetworkEnvelope> for Encoder {
    type Error = Error;

    fn encode(&mut self, envelope: NetworkEnvelope, dst: &mut BytesMut) -> Result<()> {
        let original_len = dst.len();

        // size
        dst.put_u32_le(0); // will be rewritten below.

        // protocol
        let protocol_len = envelope.message.protocol().len();
        assert!(protocol_len <= 255);
        dst.put_u8(protocol_len as u8);
        dst.extend_from_slice(envelope.message.protocol().as_bytes());

        // name
        let message_name_len = envelope.message.name().len();
        assert!(message_name_len <= 255);
        dst.put_u8(message_name_len as u8);
        dst.extend_from_slice(envelope.message.name().as_bytes());

        // trace_id
        dst.put_u64_le(u64::from(envelope.trace_id));

        // sender
        // TODO: avoid `into_remote`, transform on the caller's site.
        let sender = envelope.sender.into_remote().into_bits();
        dst.put_u64_le(sender);

        // recipient
        dst.put_u64_le(envelope.recipient.into_bits());

        // kind
        match envelope.kind {
            NetworkMessageKind::Regular => dst.put_u8(0),
            NetworkMessageKind::RequestAny(correlation_id) => {
                dst.put_u8(1);
                dst.put_u64_le(correlation_id);
            }
            NetworkMessageKind::RequestAll(correlation_id) => {
                dst.put_u8(2);
                dst.put_u64_le(correlation_id);
            }
            NetworkMessageKind::Response(correlation_id) => {
                dst.put_u8(3);
                dst.put_u64_le(correlation_id);
            }
            NetworkMessageKind::LastResponse(correlation_id) => {
                dst.put_u8(4);
                dst.put_u64_le(correlation_id);
            }
        }

        // TODO: resize buffer on failures & limit size.
        let message_size = match envelope.message.write_msgpack(&mut self.buffer) {
            Ok(size) => size,
            Err(err) => {
                dst.truncate(original_len);
                return Err(err.into());
            }
        };

        // message
        dst.extend_from_slice(&self.buffer[..message_size]);

        let size = dst.len() - original_len;
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
    type Item = NetworkEnvelope;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if src.len() < 4 {
            return Ok(None);
        }

        let size = (&src[..]).get_u32_le();
        ensure!(
            size <= self.max_frame_size,
            "frame of length {} is too large, max allowed length is {}",
            size,
            self.max_frame_size,
        );

        let size = size as usize;
        if src.len() < size {
            let additional = size - src.len();
            src.reserve(additional);
            return Ok(None);
        }

        let data = decode(&src[4..size]);
        src.advance(size);

        if let Err(err) = &data {
            // TODO: cooldown.
            tracing::error!(message = "cannot decode message, skipping", error = %err);
        }

        Ok(data.ok())
    }
}

fn decode(mut frame: &[u8]) -> Result<NetworkEnvelope> {
    let protocol = get_str(&mut frame)?;
    let name = get_str(&mut frame)?;

    let trace_id = TraceId::try_from(frame.get_u64_le())?;
    let sender = Addr::from_bits(frame.get_u64_le());
    // TODO: avoid `into_local`, transform on the caller's site.
    let recipient = Addr::from_bits(frame.get_u64_le()).into_local();

    let kind = match frame.get_u8() {
        0 => NetworkMessageKind::Regular,
        1 => NetworkMessageKind::RequestAny(frame.get_u64_le()),
        2 => NetworkMessageKind::RequestAll(frame.get_u64_le()),
        3 => NetworkMessageKind::Response(frame.get_u64_le()),
        4 => NetworkMessageKind::LastResponse(frame.get_u64_le()),
        n => bail!("invalid message kind: {n}"),
    };

    let message = AnyMessage::read_msgpack(protocol, name, frame)?
        .ok_or_else(|| eyre!("unknown message {}::{}", protocol, name))?;

    Ok(NetworkEnvelope {
        sender,
        recipient,
        trace_id,
        kind,
        message,
    })
}

fn get_str<'a>(frame: &mut &'a [u8]) -> Result<&'a str> {
    let len = usize::from(frame.get_u8());
    // TODO: It's not enough, still can fail if `len` is wrong.
    ensure!(len < frame.len(), "invalid header");
    let name = str::from_utf8(&frame[..len])?;
    frame.advance(len);
    Ok(name)
}

// === NetworkEnvelope ===

pub(crate) struct NetworkEnvelope {
    pub(crate) sender: Addr,
    pub(crate) recipient: Addr,
    pub(crate) trace_id: TraceId,
    pub(crate) kind: NetworkMessageKind,
    pub(crate) message: AnyMessage,
}

pub(crate) enum NetworkMessageKind {
    Regular,
    RequestAny(u64),
    RequestAll(u64),
    Response(u64),
    LastResponse(u64),
}

#[cfg(test)]
mod tests {
    use tokio_util::codec::{Decoder as _, Encoder as _};

    use elfo_core::message;

    use super::*;

    #[test]
    fn it_works() {
        #[message]
        #[derive(PartialEq)]
        struct Test(u64);

        let mut bytes = BytesMut::new();
        let mut encoder = Encoder::new(1024);
        let mut decoder = Decoder::new(1024);

        for i in 1..5 {
            let test = Test(i);
            let sender = Addr::NULL;
            let recipient = Addr::NULL;

            let trace_id = TraceId::try_from(i).unwrap();
            let envelope = NetworkEnvelope {
                sender,
                recipient,
                trace_id,
                kind: NetworkMessageKind::Regular,
                message: test.upcast(),
            };

            encoder.encode(envelope, &mut bytes).unwrap();

            let actual = decoder.decode(&mut bytes).unwrap().unwrap();

            assert_eq!(actual.trace_id, trace_id);
            assert_eq!(actual.sender, sender);
            assert_eq!(actual.recipient, recipient);
            assert_eq!(actual.message.downcast::<Test>().unwrap(), Test(i));
        }
    }
}
