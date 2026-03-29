use std::{convert::TryFrom, io::Cursor};

use byteorder::{LittleEndian, ReadBytesExt};
use eyre::{ensure, eyre, Error, WrapErr};
use tracing::error;

use elfo_core::{errors::RequestError, tracing::TraceId, AnyMessage, RequestId};
use elfo_utils::likely;

use crate::codec::format::{
    NetworkAddr, NetworkEnvelope, NetworkEnvelopePayload, FLAG_IS_LAST_RESPONSE,
    FLAG_IS_UNBOUNDED_SEND, KIND_MASK, KIND_REGULAR, KIND_REQUEST_ALL, KIND_REQUEST_ANY,
    KIND_RESPONSE_FAILED, KIND_RESPONSE_IGNORED, KIND_RESPONSE_OK,
};

#[derive(Default)]
pub(crate) struct DecodeStats {
    /// How many messages were decoded so far.
    pub(crate) total_messages_decoded: u64,
    /// How many messages were skipped because of non-fatal decoding errors.
    pub(crate) total_messages_decoding_skipped: u64,
}

#[derive(Debug)]
pub(crate) struct EnvelopeDetails {
    pub(crate) kind: u8,
    pub(crate) sender: NetworkAddr,
    pub(crate) recipient: NetworkAddr,
    pub(crate) request_id: Option<RequestId>,
    pub(crate) trace_id: TraceId,
}

pub(crate) enum DecodeState {
    /// Buffer needs to contain at least `total_length_estimate` bytes in total
    /// in order for the decoder to make progress.
    NeedMoreData { total_length_estimate: usize },
    /// There was a non-fatal error while decoding a message residing in
    /// `bytes_consumed` bytes, so it was skipped.
    Skipped {
        bytes_consumed: usize,
        details: Option<EnvelopeDetails>,
    },
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

    // TODO: cooldown/metrics.
    let DecodeError { message, details } = decode_result.unwrap_err();
    if let Some(details) = &details {
        error!(
            message = "cannot decode message, skipping",
            error = format!("{:#}", message.error),
            protocol = message.protocol.as_deref().unwrap_or("<unknown>"),
            name = message.name.as_deref().unwrap_or("<unknown>"),
            kind = ?details.kind,
            sender = %details.sender,
            recipient = %details.recipient,
            request_id = ?details.request_id,
            trace_id = %details.trace_id,
        );
    } else {
        error!(
            message = "cannot decode message, skipping",
            error = format!("{:#}", message.error),
            protocol = message.protocol.as_deref().unwrap_or("<unknown>"),
            name = message.name.as_deref().unwrap_or("<unknown>"),
        );
    }

    Ok(DecodeState::Skipped {
        bytes_consumed: size,
        details,
    })
}

#[derive(Debug)]
struct DecodeError {
    message: MessageDecodeError,
    details: Option<EnvelopeDetails>,
}

impl<T> From<T> for DecodeError
where
    T: Into<eyre::Report>,
{
    fn from(value: T) -> Self {
        DecodeError {
            message: MessageDecodeError {
                protocol: None,
                name: None,
                error: value.into(),
            },
            details: None,
        }
    }
}

#[derive(Debug)]
struct MessageDecodeError {
    protocol: Option<String>,
    name: Option<String>,
    error: eyre::Report,
}

impl<T> From<T> for MessageDecodeError
where
    T: Into<eyre::Report>,
{
    fn from(value: T) -> Self {
        MessageDecodeError {
            protocol: None,
            name: None,
            error: value.into(),
        }
    }
}

fn get_addr(frame: &mut Cursor<&[u8]>) -> eyre::Result<NetworkAddr> {
    let bits = frame.read_u64::<LittleEndian>()?;
    NetworkAddr::from_bits(bits).map_err(Error::msg)
}

fn get_request_id(frame: &mut Cursor<&[u8]>) -> eyre::Result<RequestId> {
    Ok(RequestId::from_ffi(frame.read_u64::<LittleEndian>()?))
}

fn get_message(frame: &mut Cursor<&[u8]>) -> Result<AnyMessage, MessageDecodeError> {
    let protocol = get_str(frame).wrap_err("invalid message protocol")?;
    let name = get_str(frame)
        .wrap_err("invalid message name")
        .map_err(|error| MessageDecodeError {
            protocol: Some(protocol.to_string()),
            name: None,
            error,
        })?;

    // TODO: replace with `Cursor::remaining_slice` once it becomes stable.
    let position = frame.position() as usize;
    let remaining_slice = &frame.get_ref()[position..];

    let result = AnyMessage::read_msgpack(remaining_slice, protocol, name).map_err(|error| {
        MessageDecodeError {
            protocol: Some(protocol.to_string()),
            name: Some(name.to_string()),
            error: error.into(),
        }
    })?;
    frame.set_position(frame.get_ref().len() as u64);

    result.ok_or_else(|| MessageDecodeError {
        protocol: Some(protocol.to_string()),
        name: Some(name.to_string()),
        error: eyre!("unknown message"),
    })
}

fn get_str<'a>(frame: &mut Cursor<&'a [u8]>) -> eyre::Result<&'a str> {
    let len = frame.read_u8()? as usize;
    let string_end = frame.position() as usize + len;

    // TODO: It's not enough, still can fail if `len` is wrong.
    ensure!(frame.get_ref().len() >= string_end, "invalid string header");

    let bytes_slice = &frame.get_ref()[frame.position() as usize..string_end];
    let decoded_string = std::str::from_utf8(bytes_slice)?;
    frame.set_position(frame.position() + len as u64);
    Ok(decoded_string)
}

fn do_decode(frame: &mut Cursor<&[u8]>) -> Result<NetworkEnvelope, DecodeError> {
    let flags = frame.read_u8()?;
    let kind = flags & KIND_MASK;

    let sender = get_addr(frame)?;
    let recipient = get_addr(frame)?;
    let trace_id = TraceId::try_from(frame.read_u64::<LittleEndian>()?)?;

    let map_decode_error = |result: Result<AnyMessage, MessageDecodeError>,
                            request_id: Option<RequestId>|
     -> Result<AnyMessage, DecodeError> {
        result.map_err(|message| DecodeError {
            message,
            details: Some(EnvelopeDetails {
                kind,
                sender,
                recipient,
                request_id,
                trace_id,
            }),
        })
    };

    use NetworkEnvelopePayload::*;
    let payload = match kind {
        KIND_REGULAR => Regular {
            message: map_decode_error(get_message(frame), None)?,
        },
        KIND_REQUEST_ANY => {
            let request_id = get_request_id(frame)?;
            RequestAny {
                request_id,
                message: map_decode_error(get_message(frame), Some(request_id))?,
            }
        }
        KIND_REQUEST_ALL => {
            let request_id = get_request_id(frame)?;
            RequestAll {
                request_id,
                message: map_decode_error(get_message(frame), Some(request_id))?,
            }
        }
        KIND_RESPONSE_OK => {
            let request_id = get_request_id(frame)?;
            Response {
                request_id,
                message: Ok(map_decode_error(get_message(frame), Some(request_id))?),
                is_last: flags & FLAG_IS_LAST_RESPONSE != 0,
            }
        }
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
        n => return Err(eyre!("invalid message kind: {n}").into()),
    };

    Ok(NetworkEnvelope {
        sender,
        recipient,
        trace_id,
        payload,
        bounded: flags & FLAG_IS_UNBOUNDED_SEND == 0,
    })
}
