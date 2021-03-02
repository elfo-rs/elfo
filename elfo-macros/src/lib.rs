use proc_macro::TokenStream;

mod message;
mod msg;

#[proc_macro]
pub fn msg(input: TokenStream) -> TokenStream {
    msg::msg_impl(input)
}

#[proc_macro_attribute]
pub fn message(attr: TokenStream, input: TokenStream) -> TokenStream {
    message::message_impl(attr, input)
}
