//! Contains `msg!` and `message!` proc-macros.

use proc_macro::TokenStream;
use syn::parse_quote;

use elfo_macros_impl::{message_impl, msg_impl};

/// Matches a message based on the provided envelope.
#[proc_macro]
pub fn msg(input: TokenStream) -> TokenStream {
    msg_impl(input, parse_quote!(::elfo))
}

#[doc(hidden)]
#[proc_macro]
pub fn msg_core(input: TokenStream) -> TokenStream {
    msg_impl(input, parse_quote!(::elfo_core))
}

/// Derives required traits to use the type as a message or a message part.
///
/// Attributes:
/// * `part` — do not derive `Message`. Useful for parts of messages.
/// * `ret = SomeType` — also derive `Request` with the provided response type.
/// * `name = "SomeName"` — override a message name.
/// * `not(Debug)` — do not derive `Debug`. Useful for custom instances.
/// * `not(Clone)` — the same for `Clone`.
/// * `elfo = some::path` — override a path to elfo.
#[proc_macro_attribute]
pub fn message(attr: TokenStream, input: TokenStream) -> TokenStream {
    message_impl(attr, input, parse_quote!(::elfo))
}

#[doc(hidden)]
#[proc_macro_attribute]
pub fn message_core(attr: TokenStream, input: TokenStream) -> TokenStream {
    message_impl(attr, input, parse_quote!(::elfo_core))
}
