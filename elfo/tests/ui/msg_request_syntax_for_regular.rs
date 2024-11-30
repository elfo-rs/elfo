use elfo::{message, msg, Envelope};

#[message]
struct SomeEvent;

fn test(envelope: Envelope) {
    msg!(match envelope {
        (SomeEvent, token) => {}
    });
}

fn main() {}
