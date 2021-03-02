use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parenthesized,
    parse::{Error as ParseError, Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    DeriveInput, Ident, Path, Token,
};

struct MessageArgs {
    responses: Vec<Path>,
}

impl Parse for MessageArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self, ParseError> {
        // `#[message]`
        if input.is_empty() {
            return Ok(MessageArgs {
                responses: Vec::new(),
            });
        }

        // `#[message(response(A, B, C))]`
        let ident: Ident = input.parse()?;
        assert_eq!(ident.to_string(), "response");

        let content;
        parenthesized!(content in input);
        let punctuated: Punctuated<Path, Token![,]> = content.parse_terminated(Path::parse)?;

        Ok(MessageArgs {
            responses: punctuated
                .into_pairs()
                .map(|pair| pair.into_value())
                .collect(),
        })
    }
}

pub fn message_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as MessageArgs);
    // TODO: what about parsing into something cheaper?
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident.clone();

    let derive_request = if !args.responses.is_empty() {
        let responses = args.responses;
        quote! {
            impl elfo::Request for #name {
                type Response = #(#responses)*;
            }
        }
    } else {
        quote! {}
    };

    // TODO: impl `Serialize` and `Deserialize`.
    TokenStream::from(quote! {
        #[derive(Clone)]
        #input

        impl elfo::Message for #name {}
        #derive_request
    })
}
