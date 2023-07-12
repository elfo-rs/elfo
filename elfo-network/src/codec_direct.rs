use std::{convert::TryFrom, io::Cursor};

use crate::codec::{
    DecoderDeltaStats, EncodeError, EncoderDeltaStats, NetworkEnvelope, NetworkEnvelopePayload,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use bytes::Buf;
use elfo_core::{
    errors::RequestError,
    scope, Addr, Message,
    _priv::{AnyMessage, RequestId},
    tracing::TraceId,
};
use elfo_utils::likely;
use eyre::{bail, ensure, eyre, Context};
use tracing::error;

/// TODO(laplab): merge with codec.rs

const FLAG_IS_LAST_RESPONSE: u8 = 1 << 7;

const KIND_MASK: u8 = 0xF;
const KIND_REGULAR: u8 = 0;
const KIND_REQUEST_ANY: u8 = 1;
const KIND_REQUEST_ALL: u8 = 2;
const KIND_RESPONSE_OK: u8 = 3;
const KIND_RESPONSE_FAILED: u8 = 4;
const KIND_RESPONSE_IGNORED: u8 = 5;

fn do_encode(
    envelope: &NetworkEnvelope,
    dst: &mut Vec<u8>,
    start_pos: usize,
    limit: Option<usize>,
) -> eyre::Result<()> {
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
    dst.write_u8(flags | kind)?;

    // sender
    // TODO: avoid `into_remote`, transform on the caller's site.
    let sender = envelope.sender.into_remote().into_bits();
    dst.write_u64::<LittleEndian>(sender)?;

    // recipient
    dst.write_u64::<LittleEndian>(envelope.recipient.into_bits())?;

    // trace_id
    dst.write_u64::<LittleEndian>(u64::from(envelope.trace_id))?;

    // request_id
    if let Some(request_id) = request_id {
        dst.write_u64::<LittleEndian>(request_id.to_ffi())?;
    }

    if let Some(message) = message {
        let mut put_str = |s: &str| -> eyre::Result<()> {
            let size = s.len();
            assert!(size <= 255);
            dst.write_u8(size as u8)?;
            dst.extend_from_slice(s.as_bytes());
            Ok(())
        };

        // protocol
        put_str(message.protocol())?;

        // name
        put_str(message.name())?;

        // message
        let max_limit = u32::MAX as usize - (dst.len() - start_pos);
        let limit = limit.map_or(max_limit, |limit| limit.min(max_limit));

        scope::with_serde_mode(scope::SerdeMode::Network, || {
            message.write_msgpack(dst, limit)
        })?;
    }

    Ok(())
}

pub(crate) fn encode(
    envelope: &NetworkEnvelope,
    dst: &mut Vec<u8>,
    stats: &mut EncoderDeltaStats,
    limit: Option<usize>,
) -> Result<(), EncodeError> {
    let start_pos = dst.len();

    // Reserve space for size, this will be rewritten below.
    dst.write_u32::<LittleEndian>(0)?;

    let res = do_encode(envelope, dst, start_pos, limit);

    if likely(res.is_ok()) {
        // Rewrite the total frame size (message + length) if encoding was successfull.
        let size = dst.len() - start_pos;
        (&mut dst[start_pos..]).write_u32::<LittleEndian>(size as u32)?;

        stats.messages += 1;
        stats.bytes += size as u64;

        return Ok(());
    }

    let err = res.unwrap_err();

    // TODO: if the limit is reached, we need also to release memory of the buffer.

    // If there was an encoding error, reset any changes to the buffer.
    dst.resize(start_pos, 0);

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

pub(crate) enum DecodeState<T> {
    NeedMoreData { length_estimate: usize },
    Done { bytes_consumed: usize, decoded: T },
}

fn get_request_id(frame: &mut Cursor<&[u8]>) -> eyre::Result<RequestId> {
    Ok(RequestId::from_ffi(frame.read_u64::<LittleEndian>()?))
}

fn get_message(frame: &mut Cursor<&[u8]>) -> eyre::Result<AnyMessage> {
    let protocol = get_str(frame).wrap_err("invalid message protocol")?;
    let name = get_str(frame).wrap_err("invalid message name")?;

    // TODO: replace with `Cursor::remaining_slice` once it becomes stable.
    let position = frame.position() as usize;
    let remaining_slice = &frame.get_ref()[position..];

    let result = AnyMessage::read_msgpack(remaining_slice, protocol, name)?;
    frame.advance(frame.get_ref().len() - position);

    result.ok_or_else(|| eyre!("unknown message {}::{}", protocol, name))
}

fn get_str<'a>(frame: &mut Cursor<&'a [u8]>) -> eyre::Result<&'a str> {
    let len = frame.read_u8()? as usize;
    let string_end = frame.position() as usize + len;

    // TODO: It's not enough, still can fail if `len` is wrong.
    ensure!(frame.get_ref().len() >= string_end, "invalid string header");

    let bytes_slice = &frame.get_ref()[frame.position() as usize..string_end];
    let decoded_string = std::str::from_utf8(bytes_slice)?;
    frame.advance(len);
    Ok(decoded_string)
}

fn do_decode(frame: &mut Cursor<&[u8]>) -> eyre::Result<NetworkEnvelope> {
    let flags = frame.read_u8()?;
    let kind = flags & KIND_MASK;

    let sender = Addr::from_bits(frame.read_u64::<LittleEndian>()?);
    // TODO: avoid `into_local`, transform on the caller's site.
    let recipient = Addr::from_bits(frame.read_u64::<LittleEndian>()?).into_local();
    let trace_id = TraceId::try_from(frame.read_u64::<LittleEndian>()?)?;

    use NetworkEnvelopePayload::*;
    let payload = match kind {
        KIND_REGULAR => Regular {
            message: get_message(frame)?,
        },
        KIND_REQUEST_ANY => RequestAny {
            request_id: get_request_id(frame)?,
            message: get_message(frame)?,
        },
        KIND_REQUEST_ALL => RequestAll {
            request_id: get_request_id(frame)?,
            message: get_message(frame)?,
        },
        KIND_RESPONSE_OK => Response {
            request_id: get_request_id(frame)?,
            message: Ok(get_message(frame)?),
            is_last: flags & FLAG_IS_LAST_RESPONSE != 0,
        },
        KIND_RESPONSE_FAILED => Response {
            request_id: get_request_id(frame)?,
            message: Err(RequestError::Failed),
            is_last: flags & FLAG_IS_LAST_RESPONSE != 0,
        },
        KIND_RESPONSE_IGNORED => Response {
            request_id: get_request_id(frame)?,
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

pub(crate) fn decode(
    input: &[u8],
    stats: &mut DecoderDeltaStats,
) -> eyre::Result<DecodeState<NetworkEnvelope>> {
    if input.len() < 4 {
        return Ok(DecodeState::NeedMoreData { length_estimate: 4 });
    }

    let mut src = Cursor::new(input);
    let size = src.read_u32::<LittleEndian>()? as usize;

    if input.len() < size {
        return Ok(DecodeState::NeedMoreData {
            length_estimate: size - input.len(),
        });
    }

    let decode_result = do_decode(&mut src);
    if likely(decode_result.is_ok()) {
        stats.messages += 1;
        stats.bytes += size as u64;
        return Ok(DecodeState::Done {
            bytes_consumed: size,
            decoded: decode_result.unwrap(),
        });
    }

    let err = decode_result.unwrap_err();
    // TODO: cooldown/metrics, more info (protocol and name if available)
    error!(
        message = "cannot decode message, skipping",
        error = format!("{:#}", err)
    );
    Err(err)
}
