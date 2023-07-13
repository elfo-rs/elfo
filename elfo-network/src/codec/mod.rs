pub(crate) mod decode;
pub(crate) mod encode;
pub(crate) mod format;

#[cfg(test)]
mod tests {
    use elfo_core::{message, tracing::TraceId, Addr, Message};
    use std::convert::TryFrom;

    use crate::codec::decode::DecodeState;

    use super::{
        decode::decode,
        encode::{encode, EncodeError},
        format::{NetworkEnvelope, NetworkEnvelopePayload},
    };

    #[test]
    fn smoke() {
        #[message]
        #[derive(PartialEq)]
        struct SmallMessage(u64);

        #[message]
        struct BigMessage(String);

        let mut bytes = Vec::new();
        let mut position = 0;

        for i in 1..5 {
            let make_envelope = |message| NetworkEnvelope {
                sender: Addr::NULL,
                recipient: Addr::NULL,
                trace_id: TraceId::try_from(i).unwrap(),
                payload: NetworkEnvelopePayload::Regular { message },
            };

            let small_envelope = make_envelope(SmallMessage(i).upcast());
            let big_envelope = make_envelope(BigMessage("oops".repeat(100)).upcast());

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

            if let NetworkEnvelopePayload::Regular { message } = decoded_small_envelope.payload {
                assert_eq!(message.downcast::<SmallMessage>().unwrap(), SmallMessage(i));
            } else {
                panic!("unexpected message kind");
            }
        }
    }

    // TODO: test errors.
}
