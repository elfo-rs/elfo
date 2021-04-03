use proc_macro::TokenStream;
use syn::parse_quote;

mod message;
mod msg;

#[proc_macro]
pub fn msg(input: TokenStream) -> TokenStream {
    msg::msg_impl(input, parse_quote!(::elfo))
}

// TODO: is it enough to have only one `msg!` instead?
#[proc_macro]
pub fn msg_raw(input: TokenStream) -> TokenStream {
    msg::msg_impl(input, parse_quote!(elfo))
}

#[proc_macro_attribute]
pub fn message(attr: TokenStream, input: TokenStream) -> TokenStream {
    message::message_impl(attr, input)
}
