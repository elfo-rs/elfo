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

use elfo_core::{
    errors::RequestError,
    tracing::TraceId,
    Addr, Message,
    _priv::{AnyMessage, RequestId},
};

// Flags are shifted by 4 bits to the left because of the kind.
pub(crate) const FLAG_IS_LAST_RESPONSE: u8 = 1 << 7;

pub(crate) const KIND_MASK: u8 = 0xF;
pub(crate) const KIND_REGULAR: u8 = 0;
pub(crate) const KIND_REQUEST_ANY: u8 = 1;
pub(crate) const KIND_REQUEST_ALL: u8 = 2;
pub(crate) const KIND_RESPONSE_OK: u8 = 3;
pub(crate) const KIND_RESPONSE_FAILED: u8 = 4;
pub(crate) const KIND_RESPONSE_IGNORED: u8 = 5;

#[derive(Debug)]
pub(crate) struct NetworkEnvelope {
    pub(crate) sender: Addr,
    pub(crate) recipient: Addr,
    pub(crate) trace_id: TraceId,
    pub(crate) payload: NetworkEnvelopePayload,
}

#[derive(Debug)]
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
    pub(crate) fn protocol_and_name(&self) -> (&'static str, &'static str) {
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
