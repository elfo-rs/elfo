use std::{
    convert::TryFrom,
    io::{self, Write},
    str,
};

use byteorder::{ReadBytesExt, WriteBytesExt, LE};
use eyre::{eyre, Result};
use static_assertions::const_assert;

use elfo_core::{
    tracing::TraceId,
    Addr, Envelope,
    _priv::{AnyMessage, MessageKind},
};

const BUFFER_DEFAULT_CAPACITY: usize = 4096;

const_assert!(std::mem::size_of::<usize>() <= std::mem::size_of::<u64>());
const_assert!(BUFFER_DEFAULT_CAPACITY >= 512);

// Structure:
// - u8  protocol len
// - **  protocol
// - u8  name len
// - **  name
// - u64 trace_id
// - u64 sender
// - u8  kind: 0 = Regular, 1 = RequestAny, 2 = RequestAll, 3 = Response
// - u64 request_id (for RequestAny, RequestAll and Response kinds)
// - u32 responses_left (for Response kind)
// - **  message
pub(crate) struct PacketBuffer {
    buffer: Vec<u8>,
}

impl Default for PacketBuffer {
    fn default() -> Self {
        Self {
            buffer: vec![0; BUFFER_DEFAULT_CAPACITY],
        }
    }
}

impl PacketBuffer {
    pub(crate) fn pack(&mut self, envelope: &Envelope) -> Result<&[u8]> {
        let message = envelope.message();

        let mut buffer = &mut self.buffer[..];

        let protocol_len = message.protocol().len();
        assert!(protocol_len <= 255);
        let _ = buffer.write_u8(protocol_len as u8);
        let _ = buffer.write(message.protocol().as_bytes());

        let name_len = message.name().len();
        assert!(name_len <= 255);
        let _ = buffer.write_u8(name_len as u8);
        let _ = buffer.write(message.name().as_bytes());

        let _ = buffer.write_u64::<LE>(u64::from(envelope.trace_id()));

        let sender = envelope.sender().into_bits() as u64;
        let _ = buffer.write_u64::<LE>(sender);

        // TODO: support requests
        let _ = buffer.write_u8(0);

        // TODO: resize buffer on failures.
        let message_size = message.write_msgpack(buffer)?;

        let left = buffer.len() - message_size;
        let size = self.buffer.len() - left;
        Ok(&self.buffer[0..size])
    }
}

pub(crate) fn unpack(mut data: &[u8]) -> Result<Envelope> {
    let protocol_len = data.read_u8()?;
    let protocol = read_slice(&mut data, usize::from(protocol_len))?;
    let protocol = str::from_utf8(protocol)?;

    let name_len = data.read_u8()?;
    let name = read_slice(&mut data, usize::from(name_len))?;
    let name = str::from_utf8(name)?;

    let trace_id = TraceId::try_from(data.read_u64::<LE>()?)?;
    let sender = Addr::from_bits(data.read_u64::<LE>()? as usize);

    // TODO: support requests
    let kind = data.read_u8()?;
    assert_eq!(kind, 0);
    let kind = MessageKind::Regular { sender };

    let message = AnyMessage::read_msgpack(protocol, name, data)?
        .ok_or_else(|| eyre!("unknown message {}::{}", protocol, name))?;

    Ok(Envelope::with_trace_id(message, kind, trace_id))
}

fn read_slice<'a>(data: &mut &'a [u8], len: usize) -> io::Result<&'a [u8]> {
    if data.len() < len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "failed to fill buffer",
        ));
    }

    let slice = &data[..len];
    *data = &data[len..];
    Ok(slice)
}

#[cfg(test)]
mod tests {
    use elfo_core::assert_msg_eq;
    use elfo_macros::message;

    use super::*;

    #[test]
    fn it_works() {
        #[message(elfo = elfo_core)]
        #[derive(PartialEq)]
        struct Test(u32);

        let test = Test(42);
        let envelope = Envelope::with_trace_id(
            AnyMessage::new(test),
            MessageKind::Regular { sender: Addr::NULL },
            TraceId::try_from(42).unwrap(),
        );

        let mut packet = PacketBuffer::default();
        let buffer = packet.pack(&envelope).unwrap();

        let actual = unpack(buffer).unwrap();

        assert_eq!(actual.trace_id(), envelope.trace_id());
        assert_eq!(actual.sender(), envelope.sender());
        assert_msg_eq!(actual, Test(42));
    }
}
