//! The structure of the network envelope:
//! ```text
//!           name           bits       presence
//! ┌───────────────────────┬────┬─────────────────────┐
//! │ size of whole frame   │ 32 │                     │
//! ├───────────────────────┼────┤                     │
//! │ flags                 │  4 │                     │ flags:
//! ├───────────────────────┼────┤                     │ - <reserved>       = 1
//! │ kind                  │  4 │                     │ - <reserved>       = 2
//! ├───────────────────────┼────┤       always        │ - <reserved>       = 4
//! │ sender                │ 64 │                     │ - is last response = 8
//! ├───────────────────────┼────┤                     │
//! │ recipient             │ 64 │                     │
//! ├───────────────────────┼────┤                     │
//! │ trace id              │ 64 │                     │ kinds:
//! ├───────────────────────┼────┼─────────────────────┤ - Regular           = 0
//! │ request id            │ 64 │ if kind != Regular  │ - RequestAny        = 1
//! ├───────────────────────┼────┼─────────────────────┤ - RequestAll        = 2
//! │ protocol's length (P) │  8 │                     │ - Response::Ok      = 3
//! ├───────────────────────┼────┤                     │ - Response::Failed  = 4
//! │ protocol              │ 8P │                     │ - Response::Ignored = 5
//! ├───────────────────────┼────┤ if kind !=          │
//! │ msg name's length (N) │  8 │ - Response::Failed  │
//! ├───────────────────────┼────┤ - Response::Ignored │
//! │ msg name              │ 8N │                     │
//! ├───────────────────────┼────┤                     │
//! │ msg payload           │rest│                     │
//! └───────────────────────┴────┴─────────────────────┘
//! ```
//!
//! All fields are encoded using LE ordering.

// TODO: send message ID instead of protocol/name.

use derive_more::Display;

use elfo_core::{
    addr::{Addr, NodeNo},
    errors::RequestError,
    tracing::TraceId,
    AnyMessage, Message, RequestId,
};
use elfo_utils::likely;

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
    pub(crate) sender: NetworkAddr,
    pub(crate) recipient: NetworkAddr,
    pub(crate) trace_id: TraceId,
    pub(crate) payload: NetworkEnvelopePayload,
}

/// A wrapper around `Addr` to ensure it's not local.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display)]
pub(crate) struct NetworkAddr(Addr);

impl NetworkAddr {
    pub(crate) const NULL: Self = Self(Addr::NULL);

    pub(crate) fn from_local(addr: Addr, node_no: NodeNo) -> Self {
        debug_assert!(!addr.is_remote());
        Self(addr.into_remote(node_no))
    }

    pub(crate) fn from_remote(addr: Addr) -> Self {
        debug_assert!(!addr.is_local());
        Self(addr)
    }

    pub(crate) fn from_bits(bits: u64) -> Result<Self, &'static str> {
        let addr = Addr::from_bits(bits).ok_or("invalid addr")?;

        if likely(!addr.is_local()) {
            Ok(Self(addr))
        } else {
            Err("addr cannot be local")
        }
    }

    pub(crate) fn into_local(self) -> Addr {
        self.0.into_local()
    }

    pub(crate) fn into_remote(self) -> Addr {
        self.0
    }

    pub(crate) fn into_bits(self) -> u64 {
        self.0.into_bits()
    }
}

// It's safe to serialize `NetworkAddr` because it's not local.
// Used by the internode protocol.
impl serde::Serialize for NetworkAddr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.into_bits().serialize(serializer)
    }
}

// It's safe to deserialize `NetworkAddr` after ensuring it's not local.
// Used by the internode protocol.
impl<'de> serde::Deserialize<'de> for NetworkAddr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let bits = u64::deserialize(deserializer)?;
        NetworkAddr::from_bits(bits).map_err(D::Error::custom)
    }
}

// TODO: use `elfo::Envelope` to avoid extra allocation with `AnyMessage`.
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
