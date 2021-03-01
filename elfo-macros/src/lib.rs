use proc_macro::TokenStream;

mod msg;

#[proc_macro]
pub fn msg(input: TokenStream) -> TokenStream {
    msg::msg_impl(input)
}
