use std::time::UNIX_EPOCH;

use proc_macro::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::{
    parse::{Error as ParseError, Parse, ParseStream},
    parse_macro_input, parse_quote, DeriveInput, Ident, Path, Token, Type,
};

#[derive(Debug)]
struct MessageArgs {
    ret: Option<Type>,
    crate_: Path,
}

impl Parse for MessageArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self, ParseError> {
        // TODO: support any order of attributes.

        let mut args = MessageArgs {
            ret: None,
            crate_: parse_quote!(::elfo),
        };

        // `#[message]`
        // `#[message(ret(A))]`
        // `#[message(ret(A), crate = some)]`
        // `#[message(crate = some::path)]`
        while !input.is_empty() {
            let ident: Ident = input.parse()?;

            match ident.to_string().as_str() {
                "ret" => {
                    let _: Token![=] = input.parse()?;
                    args.ret = Some(input.parse()?);
                }
                // TODO: call it `crate` like in linkme?
                "elfo" => {
                    let _: Token![=] = input.parse()?;
                    args.crate_ = input.parse()?;
                }
                attr => panic!("invalid attribute: {}", attr),
            }

            if !input.is_empty() {
                let _: Token![,] = input.parse()?;
            }
        }

        Ok(args)
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
    let mod_name = format_ident!("_elfo_{}", name);
    let ltid = gen_ltid();
    let serde_crate = format!("{}::_priv::serde", args.crate_.to_token_stream());
    let crate_ = args.crate_;

    let derive_request = if let Some(ret) = &args.ret {
        quote! {
            impl #crate_::Request for #name {
                type Response = #ret;
                type Wrapper = #mod_name::Wrapper;
            }
        }
    } else {
        quote! {}
    };

    let request_wrapper = if let Some(ret) = &args.ret {
        quote! {
            #[message(elfo = #crate_)] // `message` is imported in the module.
            pub struct Wrapper(#ret);

            impl From<#ret> for Wrapper {
                #[inline]
                fn from(inner: #ret) -> Self {
                    Wrapper(inner)
                }
            }

            impl From<Wrapper> for #ret {
                #[inline]
                fn from(wrapper: Wrapper) -> Self {
                    wrapper.0
                }
            }
        }
    } else {
        quote! {}
    };

    TokenStream::from(quote! {
        #[derive(Debug, Clone)]
        #[derive(#crate_::_priv::serde::Serialize, #crate_::_priv::serde::Deserialize)]
        #[serde(crate = #serde_crate)]
        #input

        impl #crate_::Message for #name {
            const _LTID: #crate_::_priv::LocalTypeId = #ltid;
        }

        #[allow(non_snake_case)]
        mod #mod_name {
            use super::*;

            use std::fmt;

            use #crate_::_priv::{MESSAGE_LIST, MessageVTable, smallbox::{smallbox}, AnyMessage, linkme};
            use #crate_::message;

            #request_wrapper

            fn cast_ref(message: &AnyMessage) -> &#name {
                message.downcast_ref::<#name>().expect("invalid vtable")
            }

            fn clone(message: &AnyMessage) -> AnyMessage {
                smallbox!(Clone::clone(cast_ref(message)))
            }

            fn debug(message: &AnyMessage, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(cast_ref(message), f)
            }

            #[linkme::distributed_slice(MESSAGE_LIST)]
            #[linkme(crate = #crate_::_priv::linkme)]
            static VTABLE: MessageVTable = MessageVTable {
                ltid: #ltid,
                clone,
                debug,
            };
        }

        #derive_request
    })
}
