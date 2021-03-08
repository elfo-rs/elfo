use std::time::UNIX_EPOCH;

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

fn gen_ltid() -> u32 {
    // TODO
    let elapsed = UNIX_EPOCH.elapsed().expect("invalid system time");
    elapsed.as_nanos() as u32
}

pub fn message_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as MessageArgs);
    // TODO: what about parsing into something cheaper?
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident.clone();
    let mod_name = Ident::new(&format!("_elfo_{}", name), name.span());
    let ltid = gen_ltid();

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

        impl ::elfo::Message for #name {
            const _LTID: elfo::_priv::LocalTypeId = #ltid;
        }

        #[allow(non_snake_case)]
        mod #mod_name {
            use super::#name;

            use ::elfo::_priv::{MESSAGE_LIST, MessageVTable, smallbox::{smallbox}, AnyMessage, linkme};

            fn clone(message: &AnyMessage) -> AnyMessage {
                smallbox!(message.downcast_ref::<#name>().expect("invalid vtable").clone())
            }

            #[linkme::distributed_slice(MESSAGE_LIST)]
            #[linkme(crate = elfo::_priv::linkme)]
            static VTABLE: MessageVTable = MessageVTable {
                ltid: #ltid,
                clone,
            };
        }

        #derive_request
    })
}
