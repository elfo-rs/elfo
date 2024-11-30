use elfo::{msg, Envelope};

// TODO: check more invalid patterns.
fn test(envelope: Envelope) {
    msg!(match envelope {
        foo!() => {}
        "liternal" => {}
        10..20 => {}
        (A | B) => {}
        (SomeRequest, token, extra) => {}
        (SomeRequest, 20) => {}
    });
}

fn main() {}
