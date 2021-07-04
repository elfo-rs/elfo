use elfo::{message, Message, Request};
use serde::Serialize;
use static_assertions::*;

#[message]
struct SimpleMessage {}
assert_impl_all!(SimpleMessage: Message);
assert_not_impl_all!(SimpleMessage: Request);

#[message(ret = ())]
struct SimpleRequest {}
assert_impl_all!(SimpleRequest: Message, Request);

#[message(part)]
struct SimpleMessagePart(u32);
assert_impl_all!(SimpleMessagePart: std::fmt::Debug, Serialize);
assert_not_impl_all!(SimpleMessagePart: Message, Request);

#[message(part, transparent)]
struct TransparentMessagePart(u32);
assert_impl_all!(TransparentMessagePart: std::fmt::Debug, Serialize);
assert_not_impl_all!(TransparentMessagePart: Message, Request);

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
    assert_eq!(SimpleMessage::NAME, "SimpleMessage");
    assert_eq!(SimpleRequest::NAME, "SimpleRequest");
}
