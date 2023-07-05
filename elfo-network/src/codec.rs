//! This module contains `Encoder` and `Decoder` instances.
//!
//! The structure of the network envelope:
//!           name           bits       presence
//! +-----------------------+----+---------------------+
//! | size of whole frame   | 32 |                     |
//! +-----------------------+----+                     |
//! | flags                 |  4 |                     | flags:
//! +-----------------------+----+                     | - <reserved>       = 1
//! | kind                  |  4 |                     | - <reserved>       = 2
//! +-----------------------+----+       always        | - <reserved>       = 4
//! | sender                | 64 |                     | - is last response = 8
//! +-----------------------+----+                     |
//! | recipient             | 64 |                     |
//! +-----------------------+----+                     |
//! | trace id              | 64 |                     | kinds:
//! +-----------------------+----+---------------------+ - Regular           = 0
//! | request id            | 64 | if kind != Regular  | - RequestAny        = 1
//! +-----------------------+----+---------------------+ - RequestAll        = 2
//! | protocol's length (P) |  8 |                     | - Response::Ok      = 3
//! +-----------------------+----+                     | - Response::Failed  = 4
//! | protocol              | 8P |                     | - Response::Ignored = 5
//! +-----------------------+----+ if kind !=          |
//! | msg name's length (N) |  8 | - Response::Failed  |
//! +-----------------------+----+ - Response::Ignored |
//! | msg name              | 8N |                     |
//! +-----------------------+----+                     |
//! | msg payload           |rest|                     |
//! +-----------------------+----+---------------------+
//!
//! All fields are encoded using LE ordering.

// TODO: send message ID instead of protocol/name.

use std::{convert::TryFrom, mem, str};

use bytes::{Buf, BufMut, BytesMut};
use derive_more::From;
use eyre::{bail, ensure, eyre, Error, Result};
use tokio_util::codec;
use tracing::error;

use elfo_core::{
    errors::RequestError,
    scope,
    tracing::TraceId,
    Addr, Message,
    _priv::{AnyMessage, RequestId},
};
use elfo_utils::likely;

// Flags are shifted by 4 bits to the left because of the kind.
const FLAG_IS_LAST_RESPONSE: u8 = 1 << 7;

const KIND_MASK: u8 = 0xF;
const KIND_REGULAR: u8 = 0;
const KIND_REQUEST_ANY: u8 = 1;
const KIND_REQUEST_ALL: u8 = 2;
const KIND_RESPONSE_OK: u8 = 3;
const KIND_RESPONSE_FAILED: u8 = 4;
const KIND_RESPONSE_IGNORED: u8 = 5;

// === Encoder ===

const BUFFER_INITIAL_CAPACITY: usize = 8192;

pub(crate) struct Encoder {
    /// A dedicated buffer to serialize a message. Serialization into `Vec<u8>`
    /// with following extending `BytesMut` works twice faster than
    /// serialization directly into `BytesMut` according to micro-benchmarks.
    buffer: Vec<u8>,
    limit: Option<usize>,
    stats: EncoderDeltaStats,
}

#[derive(Default)]
pub(crate) struct EncoderDeltaStats {
    pub(crate) messages: u64,
    pub(crate) bytes: u64,
}

impl Encoder {
    pub(crate) fn new() -> Self {
        Self::with_limit(None)
    }

    fn with_limit(limit: Option<usize>) -> Self {
        Self {
            buffer: Vec::with_capacity(BUFFER_INITIAL_CAPACITY),
            limit,
            stats: EncoderDeltaStats::default(),
        }
    }

    pub(crate) fn take_stats(&mut self) -> EncoderDeltaStats {
        mem::take(&mut self.stats)
    }

    fn do_encode(
        &mut self,
        envelope: &NetworkEnvelope,
        dst: &mut BytesMut,
        start_pos: usize,
    ) -> Result<()> {
        use NetworkEnvelopePayload::*;
        let (is_last_response, kind, request_id, message) = match &envelope.payload {
            Regular { message } => (false, KIND_REGULAR, None, Some(message)),
            RequestAny {
                request_id,
                message,
            } => (false, KIND_REQUEST_ANY, Some(*request_id), Some(message)),
            RequestAll {
                request_id,
                message,
            } => (false, KIND_REQUEST_ALL, Some(*request_id), Some(message)),
            Response {
                request_id,
                message,
                is_last,
            } => (
                *is_last,
                match &message {
                    Ok(_) => KIND_RESPONSE_OK,
                    Err(RequestError::Failed) => KIND_RESPONSE_FAILED,
                    Err(RequestError::Ignored) => KIND_RESPONSE_IGNORED,
                },
                Some(*request_id),
                message.as_ref().ok(),
            ),
        };

        // flags and kind
        let mut flags = 0;
        if is_last_response {
            flags |= FLAG_IS_LAST_RESPONSE;
        }
        dst.put_u8(flags | kind);

        // sender
        // TODO: avoid `into_remote`, transform on the caller's site.
        let sender = envelope.sender.into_remote().into_bits();
        dst.put_u64_le(sender);

        // recipient
        dst.put_u64_le(envelope.recipient.into_bits());

        // trace_id
        dst.put_u64_le(u64::from(envelope.trace_id));

        // request_id
        if let Some(request_id) = request_id {
            dst.put_u64_le(request_id.to_ffi());
        }

        if let Some(message) = message {
            // protocol
            put_str(dst, message.protocol());

            // name
            put_str(dst, message.name());

            // message
            let max_limit = u32::MAX as usize - (dst.len() - start_pos);
            let limit = self.limit.map_or(max_limit, |limit| limit.min(max_limit));
            self.buffer.clear();

            scope::with_serde_mode(scope::SerdeMode::Network, || {
                message.write_msgpack(&mut self.buffer, limit)
            })?;

            dst.extend_from_slice(&self.buffer);
        }

        Ok(())
    }
}

#[derive(Debug, From)]
pub(crate) enum EncodeError {
    Fatal(std::io::Error),
    Skipped,
}

impl codec::Encoder<NetworkEnvelope> for Encoder {
    type Error = EncodeError;

    fn encode(&mut self, envelope: NetworkEnvelope, dst: &mut BytesMut) -> Result<(), EncodeError> {
        let start_pos = dst.len();

        // size
        dst.put_u32_le(0); // will be rewritten below.

        let res = self.do_encode(&envelope, dst, start_pos);

        if likely(res.is_ok()) {
            let size = dst.len() - start_pos;
            (&mut dst[start_pos..]).put_u32_le(size as u32);

            self.stats.messages += 1;
            self.stats.bytes += size as u64;

            return Ok(());
        }

        let err = res.unwrap_err();
        // TODO: if the limit is reached, we need also to release memory of the buffer.

        dst.truncate(start_pos);

        // TODO: cooldown/metrics
        let (protocol, name) = envelope.payload.protocol_and_name();
        error!(
            message = "cannot encode message, skipping",
            error = %err,
            protocol = %protocol,
            name = %name,
        );
        Err(EncodeError::Skipped)
    }
}

fn put_str(dst: &mut BytesMut, s: &str) {
    let size = s.len();
    assert!(size <= 255);
    dst.put_u8(size as u8);
    dst.extend_from_slice(s.as_bytes());
}

// === Decoder ===

pub(crate) struct Decoder {
    stats: DecoderDeltaStats,
}

#[derive(Default)]
pub(crate) struct DecoderDeltaStats {
    pub(crate) messages: u64,
    pub(crate) bytes: u64,
}

impl Decoder {
    pub(crate) fn new() -> Self {
        Self {
            stats: DecoderDeltaStats::default(),
        }
    }

    pub(crate) fn take_stats(&mut self) -> DecoderDeltaStats {
        mem::take(&mut self.stats)
    }
}

impl codec::Decoder for Decoder {
    type Error = Error;
    type Item = NetworkEnvelope;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if src.len() < 4 {
            return Ok(None);
        }

        let size = (&src[..]).get_u32_le() as usize;
        if src.len() < size {
            let additional = size - src.len();
            src.reserve(additional);
            return Ok(None);
        }

        let data = decode(&src[4..size]);
        src.advance(size);

        self.stats.messages += 1;
        self.stats.bytes += size as u64;

        if let Err(err) = &data {
            // TODO: cooldown/metrics, more info (protocol and name if available)
            error!(message = "cannot decode message, skipping", error = %err);
        }

        Ok(data.ok())
    }
}

fn decode(mut frame: &[u8]) -> Result<NetworkEnvelope> {
    let flags = frame.get_u8();
    let kind = flags & KIND_MASK;

    let sender = Addr::from_bits(frame.get_u64_le());
    // TODO: avoid `into_local`, transform on the caller's site.
    let recipient = Addr::from_bits(frame.get_u64_le()).into_local();
    let trace_id = TraceId::try_from(frame.get_u64_le())?;

    use NetworkEnvelopePayload::*;
    let payload = match kind {
        KIND_REGULAR => Regular {
            message: get_message(&mut frame)?,
        },
        KIND_REQUEST_ANY => RequestAny {
            request_id: get_request_id(&mut frame),
            message: get_message(&mut frame)?,
        },
        KIND_REQUEST_ALL => RequestAll {
            request_id: get_request_id(&mut frame),
            message: get_message(&mut frame)?,
        },
        KIND_RESPONSE_OK => Response {
            request_id: get_request_id(&mut frame),
            message: Ok(get_message(&mut frame)?),
            is_last: flags & FLAG_IS_LAST_RESPONSE != 0,
        },
        KIND_RESPONSE_FAILED => Response {
            request_id: get_request_id(&mut frame),
            message: Err(RequestError::Failed),
            is_last: flags & FLAG_IS_LAST_RESPONSE != 0,
        },
        KIND_RESPONSE_IGNORED => Response {
            request_id: get_request_id(&mut frame),
            message: Err(RequestError::Ignored),
            is_last: flags & FLAG_IS_LAST_RESPONSE != 0,
        },
        n => bail!("invalid message kind: {n}"),
    };

    Ok(NetworkEnvelope {
        sender,
        recipient,
        trace_id,
        payload,
    })
}

fn get_request_id(frame: &mut &[u8]) -> RequestId {
    RequestId::from_ffi(frame.get_u64_le())
}

fn get_message(frame: &mut &[u8]) -> Result<AnyMessage> {
    let protocol = get_str(frame)?;
    let name = get_str(frame)?;
    AnyMessage::read_msgpack(frame, protocol, name)?
        .ok_or_else(|| eyre!("unknown message {}::{}", protocol, name))
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
    pub(crate) payload: NetworkEnvelopePayload,
}

pub(crate) enum NetworkEnvelopePayload {
    Regular {
        message: AnyMessage,
    },
    RequestAny {
        request_id: RequestId,
        message: AnyMessage,
    },
    RequestAll {
        request_id: RequestId,
        message: AnyMessage,
    },
    Response {
        request_id: RequestId,
        message: Result<AnyMessage, RequestError>,
        is_last: bool,
    },
}

impl NetworkEnvelopePayload {
    fn protocol_and_name(&self) -> (&'static str, &'static str) {
        match self {
            Self::Regular { message } => (message.protocol(), message.name()),
            Self::RequestAny { message, .. } => (message.protocol(), message.name()),
            Self::RequestAll { message, .. } => (message.protocol(), message.name()),
            Self::Response {
                message: Ok(message),
                ..
            } => (message.protocol(), message.name()),
            Self::Response {
                message: Err(RequestError::Failed),
                ..
            } => ("", "RequestError::Failed"),
            Self::Response {
                message: Err(RequestError::Ignored),
                ..
            } => ("", "RequestError::Ignored"),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio_util::codec::{Decoder as _, Encoder as _};

    use elfo_core::message;

    use super::*;

    #[test]
    fn smoke() {
        #[message]
        #[derive(PartialEq)]
        struct Test(u64);

        #[message]
        struct Big(String);

        let mut bytes = BytesMut::new();
        let mut encoder = Encoder::with_limit(Some(100));
        let mut decoder = Decoder::new();

        for i in 1..5 {
            let test = Test(i);
            let sender = Addr::NULL;
            let recipient = Addr::NULL;

            let trace_id = TraceId::try_from(i).unwrap();

            // Regular message.
            let envelope = NetworkEnvelope {
                sender,
                recipient,
                trace_id,
                payload: NetworkEnvelopePayload::Regular {
                    message: test.upcast(),
                },
            };
            encoder.encode(envelope, &mut bytes).unwrap();

            // A big message.
            let envelope = NetworkEnvelope {
                sender,
                recipient,
                trace_id,
                payload: NetworkEnvelopePayload::Regular {
                    message: Big("oops".repeat(100)).upcast(),
                },
            };
            assert!(matches!(
                encoder.encode(envelope, &mut bytes,).unwrap_err(),
                EncodeError::Skipped
            ));

            let actual = decoder.decode(&mut bytes).unwrap().unwrap();

            assert_eq!(actual.trace_id, trace_id);
            assert_eq!(actual.sender, sender);
            assert_eq!(actual.recipient, recipient);

            if let NetworkEnvelopePayload::Regular { message } = actual.payload {
                assert_eq!(message.downcast::<Test>().unwrap(), Test(i));
            } else {
                panic!("unexpected message kind");
            }
        }
    }

    // TODO: test errors.
}
