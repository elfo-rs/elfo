use elfo::{prelude::*, Envelope};

#[message(response(Num))]
#[derive(Debug, PartialEq)]
pub struct Num(pub u32);

#[message]
#[derive(Debug, PartialEq)]
pub struct Num2 {
    pub inner: u32,
}

// TODO
fn test_ref(envelope: &Envelope) {
    let num = msg!(match envelope {
        Num(x) if x > &4 => Num(0),
        Num(n) => Num(*n),
        _ => Num(0),
    });

    assert_eq!(num, Num(5));
}

fn test(ctx: Context<(), ()>, envelope: elfo::Envelope) {
    let num = msg!(match envelope {
        // TODO
        // Num(x) if x > 4 => Num(0),
        n @ Num2 { .. } => Num(n.inner),
        // TODO
        // n @ Num2 => Num(n.inner),
        (num @ Num(_), token) => {
            let _ = ctx.reply(token, num.clone());
            num
        }
        _ => Num(0),
    });

    assert_eq!(num, Num(5));
}

fn main() {}
