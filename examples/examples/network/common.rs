use elfo::prelude::*;

#[message(ret = String)]
pub struct AskName;

#[message]
pub struct Hello(pub u32);
