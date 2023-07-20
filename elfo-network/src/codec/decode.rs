use crate::codec::format::{
    NetworkEnvelope, NetworkEnvelopePayload, FLAG_IS_LAST_RESPONSE, KIND_MASK, KIND_REGULAR,
    KIND_REQUEST_ALL, KIND_REQUEST_ANY, KIND_RESPONSE_FAILED, KIND_RESPONSE_IGNORED,
    KIND_RESPONSE_OK,
};

use std::{convert::TryFrom, io::Cursor};

use byteorder::{LittleEndian, ReadBytesExt};
use bytes::Buf;
use elfo_core::{
    errors::RequestError,
    Addr,
    _priv::{AnyMessage, RequestId},
    tracing::TraceId,
};
use elfo_utils::likely;
use eyre::{bail, ensure, eyre, WrapErr};
use tracing::error;

#[derive(Default)]
pub(crate) struct DecodeStats {
    /// How many messages were decoded so far.
    pub(crate) total_messages_decoded: u64,
    /// How many messages were skipped because of non-fatal decoding errors.
    pub(crate) total_messages_decoding_skipped: u64,
}

pub(crate) enum DecodeState {
    /// Buffer needs to contain at least `total_length_estimate` bytes in total
    /// in order for the decoder to make progress.
    NeedMoreData { total_length_estimate: usize },
    /// There was a non-fatal error while decoding a message residing in
    /// `bytes_consumed` bytes, so it was skipped.
    Skipped { bytes_consumed: usize },
    /// Decoder decoded a value, which occupied `bytes_consumed` bytes in the
    /// buffer.
    Done {
        bytes_consumed: usize,
        decoded: NetworkEnvelope,
    },
}

pub(crate) fn decode(input: &[u8], stats: &mut DecodeStats) -> eyre::Result<DecodeState> {
    if input.len() < 4 {
        return Ok(DecodeState::NeedMoreData {
            total_length_estimate: 4,
        });
    }

    let mut src = Cursor::new(input);
    let size = src.read_u32::<LittleEndian>()? as usize;

    if input.len() < size {
        return Ok(DecodeState::NeedMoreData {
            total_length_estimate: size,
        });
    }

    let decode_result = do_decode(&mut src);
    if likely(decode_result.is_ok()) {
        stats.total_messages_decoded += 1;
        return Ok(DecodeState::Done {
            bytes_consumed: size,
            decoded: decode_result.unwrap(),
        });
    }

    stats.total_messages_decoding_skipped += 1;

    let err = decode_result.unwrap_err();
    // TODO: cooldown/metrics, more info (protocol and name if available)
    error!(
        message = "cannot decode message, skipping",
        error = format!("{:#}", err)
    );
    Ok(DecodeState::Skipped {
        bytes_consumed: size,
    })
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
