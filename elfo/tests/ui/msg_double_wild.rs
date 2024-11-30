use elfo::{messages::Terminate, msg, Envelope};

fn test(envelope: Envelope) {
    msg!(match envelope {
        Terminate => {}
        a => {}
        b => {}
    });
}

fn main() {}
