use elfo_core::{msg, Envelope, Message, Request, ResponseToken};

/// Extracts message with the provided type from [`Envelope`], panics otherwise.
#[track_caller]
pub fn extract_message<M: Message>(envelope: Envelope) -> M {
    msg!(match envelope {
        msg @ M => msg,
        msg => panic!(
            r#"expected {}, got {:#?}"#,
            elfo_core::dumping::extract_name_by_type::<M>(),
            msg.message()
        ),
    })
}

/// Extracts request message with the provided type from [`Envelope`], panics
/// otherwise.
#[track_caller]
pub fn extract_request<R: Request>(envelope: Envelope) -> (R, ResponseToken<R>) {
    msg!(match envelope {
        (req @ R, token) => (req, token),
        msg => panic!(
            r#"expected {}, got {:#?}"#,
            elfo_core::dumping::extract_name_by_type::<R>(),
            msg.message()
        ),
    })
}

#[cfg(test)]
mod tests {
    use elfo_core::{_priv::MessageKind, message, scope::Scope, ActorMeta, Addr};

    use super::*;

    #[message]
    #[derive(PartialEq)]
    struct TestMessage;

    #[message(ret = ())]
    #[derive(PartialEq)]
    struct TestRequest;

    #[test]
    fn extract_message_test() {
        create_scope().sync_within(|| {
            let envelop = Envelope::new(TestMessage, MessageKind::regular(Addr::NULL));
            let resp = extract_message::<TestMessage>(envelop);
            assert_eq!(resp, TestMessage);
        });
    }

    #[test]
    #[should_panic(expected = "expected TestMessage, got TestRequest")]
    fn extract_message_panic_test() {
        create_scope().sync_within(|| {
            let envelop = Envelope::new(TestRequest, MessageKind::regular(Addr::NULL));
            extract_message::<TestMessage>(envelop);
        });
    }

    #[test]
    fn extract_request_test() {
        create_scope().sync_within(|| {
            let envelop = Envelope::new(TestRequest, MessageKind::regular(Addr::NULL));
            let (resp, _token) = extract_request::<TestRequest>(envelop);
            assert_eq!(resp, TestRequest);
        });
    }

    fn create_scope() -> Scope {
        Scope::test(
            Addr::NULL,
            ActorMeta {
                group: "group".into(),
                key: "key".into(),
            }
            .into(),
        )
    }
}
