use elfo::prelude::*;

#[message(ret = String)]
pub struct AskName(pub u32);

#[message]
pub struct Hello(pub u32);
