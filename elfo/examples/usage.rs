use elfo::{message, msg, Context, Envelope};

#[message(response(Num))]
#[derive(Debug, PartialEq)]
pub struct Num(pub u32);

#[message]
#[derive(Debug, PartialEq)]
pub struct Num2(pub u32);

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
        Num2(x) if x > 4 => Num(0),
        Num2(n) => Num(n),
        (num @ Num(_), token) => {
            let _ = ctx.reply(token, num.clone());
            num
        }
        _ => Num(0),
    });

    assert_eq!(num, Num(5));
}

fn main() {}
