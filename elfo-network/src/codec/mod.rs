pub(crate) mod decode;
pub(crate) mod encode;
pub(crate) mod format;

#[cfg(test)]
mod tests {
    use elfo_core::{message, tracing::TraceId, Message, _priv::AnyMessage};
    use std::convert::TryFrom;

    use super::{
        decode::{decode, DecodeState},
        encode::{encode, EncodeError},
        format::{NetworkAddr, NetworkEnvelope, NetworkEnvelopePayload},
    };

    #[message]
    #[derive(PartialEq)]
    struct SmallMessage(u64);

    #[message]
    #[derive(PartialEq)]
    struct BigMessage(String);

    fn make_envelope(message: AnyMessage, trace_index: u64) -> NetworkEnvelope {
        NetworkEnvelope {
            sender: NetworkAddr::NULL,
            recipient: NetworkAddr::NULL,
            trace_id: TraceId::try_from(trace_index).unwrap(),
            payload: NetworkEnvelopePayload::Regular { message },
        }
    }

    fn assert_regular_eq<M: Message + PartialEq>(left: &NetworkEnvelope, right: &NetworkEnvelope) {
        if let NetworkEnvelopePayload::Regular { message: left } = &left.payload {
            if let NetworkEnvelopePayload::Regular { message: right } = &right.payload {
                assert_eq!(
                    left.downcast_ref::<M>().unwrap(),
                    right.downcast_ref::<M>().unwrap()
                );
            } else {
                panic!("right is not a regular message");
            }
        } else {
            panic!("left is not a regular message");
        }
    }

    #[test]
    fn smoke() {
        let mut bytes = Vec::new();
        let mut position = 0;

        for i in 1..5 {
            let small_envelope = make_envelope(SmallMessage(i).upcast(), i);
            let big_envelope = make_envelope(BigMessage("oops".repeat(100)).upcast(), i);

            // Small message must fit into 100 bytes, but big message must not.
            const LIMIT: Option<usize> = Some(100);
            let encode_start = bytes.len();
            encode(&small_envelope, &mut bytes, &mut Default::default(), LIMIT).unwrap();
            let encode_end = bytes.len();
            assert!(matches!(
                encode(&big_envelope, &mut bytes, &mut Default::default(), LIMIT).unwrap_err(),
                EncodeError::Skipped
            ));

            // Ensure that decoding that ended with error did not write anything into the
            // buffer.
            assert_eq!(encode_end, bytes.len());

            let decode_state = decode(&bytes[position..], &mut Default::default()).unwrap();
            let decoded_small_envelope = match decode_state {
                DecodeState::Skipped { .. } => {
                    panic!("there was a non-fatal error when decoding a message");
                }
                DecodeState::NeedMoreData { .. } => {
                    panic!("decoder requested more data that there was available")
                }
                DecodeState::Done {
                    bytes_consumed,
                    decoded,
                } => {
                    // Ensure that decoder read the whole message.
                    assert_eq!(encode_end - encode_start, bytes_consumed);
                    position += bytes_consumed;
                    decoded
                }
            };

            // Check that the message was decoded correctly.
            assert_eq!(decoded_small_envelope.trace_id, small_envelope.trace_id);
            assert_eq!(decoded_small_envelope.sender, small_envelope.sender);
            assert_eq!(decoded_small_envelope.recipient, small_envelope.recipient);

            assert_regular_eq::<SmallMessage>(&decoded_small_envelope, &small_envelope);
        }
    }

    #[test]
    fn test_decode_skip() {
        let mut bytes = Vec::new();

        let envelope = make_envelope(BigMessage("a".repeat(100)).upcast(), 1);

        // Encode two messages.
        encode(&envelope, &mut bytes, &mut Default::default(), None).unwrap();
        let message_size = bytes.len();
        encode(&envelope, &mut bytes, &mut Default::default(), None).unwrap();

        // Corrupt the second message.
        for byte in &mut bytes[message_size + 4..] {
            *byte = 0;
        }

        // Encode the third message on top of the corrupted first one.
        encode(&envelope, &mut bytes, &mut Default::default(), None).unwrap();

        let state = decode(&bytes, &mut Default::default()).unwrap();
        if let DecodeState::Done {
            bytes_consumed,
            decoded,
        } = state
        {
            assert_eq!(bytes_consumed, message_size);
            assert_regular_eq::<BigMessage>(&decoded, &envelope);
        } else {
            panic!("expected the first message to be decoded successfully");
        }

        let state = decode(&bytes[message_size..], &mut Default::default()).unwrap();
        if let DecodeState::Skipped { bytes_consumed, .. } = state {
            assert_eq!(bytes_consumed, message_size);
        } else {
            panic!("expected the second message to be skipped");
        }

        let state = decode(&bytes[2 * message_size..], &mut Default::default()).unwrap();
        if let DecodeState::Done {
            bytes_consumed,
            decoded,
        } = state
        {
            assert_eq!(bytes_consumed, message_size);
            assert_regular_eq::<BigMessage>(&decoded, &envelope);
        } else {
            panic!("expected the third message to be decoded successfully");
        }
    }

    // TODO: test errors (including mismatch node_no).
}
