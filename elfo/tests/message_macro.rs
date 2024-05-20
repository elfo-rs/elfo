use serde::Serialize;
use static_assertions::*;

use elfo::{message, set_protocol, Message, Request};

#[message]
struct SimpleMessage {}
assert_impl_all!(SimpleMessage: Message);
assert_not_impl_all!(SimpleMessage: Request);

#[message(ret = ())]
struct SimpleRequest {}
assert_impl_all!(SimpleRequest: Message, Request);
assert_impl_all!(<SimpleRequest as Request>::Wrapper: Message);
assert_not_impl_all!(<SimpleRequest as Request>::Wrapper: Request);

#[message(part)]
struct SimpleMessagePart(u32);
assert_impl_all!(SimpleMessagePart: std::fmt::Debug, Serialize);
assert_not_impl_all!(SimpleMessagePart: Message, Request);

#[message(part, transparent)]
struct TransparentMessagePart(u32);
assert_impl_all!(TransparentMessagePart: std::fmt::Debug, Serialize);
assert_not_impl_all!(TransparentMessagePart: Message, Request);

#[message(protocol = "override")]
struct SimpleMessageWithOverridedProtocol {}

#[message(protocol = "override", ret = ())]
struct SimpleRequestWithOverridedProtocol {}

mod one {
    use super::*;

    set_protocol!("one");

    #[message]
    pub(crate) struct SimpleMessage {}

    #[message(ret = ())]
    pub(crate) struct SimpleRequest {}

    pub(crate) mod two {
        use super::*;

        #[message]
        pub(crate) struct SimpleMessage2 {}
    }

    pub(crate) mod three {
        elfo::set_protocol!("three");

        #[elfo::message]
        pub(crate) struct SimpleMessage {}
    }
}

#[test]
fn transparent() {
    assert_eq!(
        format!("{:?}", SimpleMessagePart(42)),
        "SimpleMessagePart(42)"
    );
    assert_eq!(format!("{:?}", TransparentMessagePart(42)), "42");
}

#[test]
fn name() {
    assert_eq!(SimpleMessage {}.name(), "SimpleMessage");
    assert_eq!(SimpleRequest {}.name(), "SimpleRequest");
    assert_eq!(
        <SimpleRequest as Request>::Wrapper::from(()).name(),
        "SimpleRequest::Response"
    );
    assert_eq!(
        SimpleMessageWithOverridedProtocol {}.name(),
        "SimpleMessageWithOverridedProtocol"
    );
    assert_eq!(one::SimpleMessage {}.name(), "SimpleMessage");
    assert_eq!(one::SimpleRequest {}.name(), "SimpleRequest");
    assert_eq!(one::two::SimpleMessage2 {}.name(), "SimpleMessage2");
    assert_eq!(one::three::SimpleMessage {}.name(), "SimpleMessage");
    assert_eq!(elfo::messages::Ping::default().name(), "Ping");
}

#[test]
fn protocol() {
    assert_eq!(SimpleMessage {}.protocol(), "elfo");
    assert_eq!(SimpleRequest {}.protocol(), "elfo");
    assert_eq!(
        <SimpleRequest as Request>::Wrapper::from(()).protocol(),
        "elfo"
    );
    assert_eq!(SimpleMessageWithOverridedProtocol {}.protocol(), "override");
    assert_eq!(SimpleRequestWithOverridedProtocol {}.protocol(), "override");
    assert_eq!(
        <SimpleRequestWithOverridedProtocol as Request>::Wrapper::from(()).protocol(),
        "override"
    );
    assert_eq!(one::SimpleMessage {}.protocol(), "one");
    assert_eq!(one::SimpleRequest {}.protocol(), "one");
    assert_eq!(
        <one::SimpleRequest as Request>::Wrapper::from(()).protocol(),
        "one"
    );
    assert_eq!(one::two::SimpleMessage2 {}.protocol(), "one");
    assert_eq!(one::three::SimpleMessage {}.protocol(), "three");
    assert_eq!(elfo::messages::Ping::default().protocol(), "elfo-core");
}

#[test]
fn uniqueness() {
    // Duplicate message definition.
    #[message]
    struct SimpleMessage {}

    assert!(elfo::init::check_messages_uniqueness().is_err());
}
