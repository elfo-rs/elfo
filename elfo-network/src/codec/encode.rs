use byteorder::{LittleEndian, WriteBytesExt};
use derive_more::{Display, From};
use tracing::error;

use elfo_core::{errors::RequestError, scope, Message};
use elfo_utils::likely;

use crate::codec::format::{
    NetworkEnvelope, NetworkEnvelopePayload, FLAG_IS_LAST_RESPONSE, KIND_REGULAR, KIND_REQUEST_ALL,
    KIND_REQUEST_ANY, KIND_RESPONSE_FAILED, KIND_RESPONSE_IGNORED, KIND_RESPONSE_OK,
};

#[derive(Debug, Display, From)]
pub(crate) enum EncodeError {
    Fatal(std::io::Error),
    Skipped,
}

#[derive(Default)]
pub(crate) struct EncodeStats {
    /// How many messages were encoded so far.
    pub(crate) total_messages_encoded: u64,
    /// How many messages were skipped because of non-fatal encoding errors.
    pub(crate) total_messages_encoding_skipped: u64,
}

pub(crate) fn encode(
    envelope: &NetworkEnvelope,
    dst: &mut Vec<u8>,
    stats: &mut EncodeStats,
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

        stats.total_messages_encoded += 1;

        return Ok(());
    }

    stats.total_messages_encoding_skipped += 1;

    let error = res.unwrap_err();

    // TODO: if the limit is reached, we need also to release memory of the buffer.

    // If there was an encoding error, reset any changes to the buffer.
    dst.truncate(start_pos);

    // TODO: cooldown/metrics
    let (protocol, name) = envelope.payload.protocol_and_name();
    error!(
        message = "cannot encode message, skipping",
        error = format!("{:#}", error),
        %protocol,
        %name,
    );
    Err(EncodeError::Skipped)
}

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
    dst.write_u64::<LittleEndian>(envelope.sender.into_bits())?;

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
