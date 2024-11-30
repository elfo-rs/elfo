use elfo::{message, msg, Envelope};

#[message(ret = u32)]
struct SomeRequest;

fn test(envelope: Envelope) {
    msg!(match envelope {
        (SomeRequest, _token) => {}
    });
}

fn main() {}
