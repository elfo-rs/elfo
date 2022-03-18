use std::time::UNIX_EPOCH;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, ToTokens};
use syn::{
    parenthesized,
    parse::{Error as ParseError, Parse, ParseStream},
    parse_macro_input, parse_quote, Data, DeriveInput, Ident, LitStr, Path, Token, Type,
};

#[derive(Debug)]
struct MessageArgs {
    name: Option<LitStr>,
    ret: Option<Type>,
    part: bool,
    transparent: bool,
    dumping_allowed: bool,
    crate_: Path,
    not: Vec<String>,
}

impl Parse for MessageArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self, ParseError> {
        let mut args = MessageArgs {
            ret: None,
            name: None,
            part: false,
            transparent: false,
            dumping_allowed: true,
            crate_: parse_quote!(::elfo),
            not: Vec::new(),
        };

        // `#[message]`
        // `#[message(name = "N")]`
        // `#[message(ret = A)]`
        // `#[message(part)]`
        // `#[message(part, transparent)]`
        // `#[message(elfo = some)]`
        // `#[message(not(Debug))]`
        // `#[message(dumping = "disabled")]`
        while !input.is_empty() {
            let ident: Ident = input.parse()?;

            match ident.to_string().as_str() {
                "name" => {
                    let _: Token![=] = input.parse()?;
                    args.name = Some(input.parse()?);
                }
                "ret" => {
                    let _: Token![=] = input.parse()?;
                    args.ret = Some(input.parse()?);
                }
                "part" => args.part = true,
                "transparent" => args.transparent = true,
                "dumping" => {
                    // TODO: introduce `DumpingMode`.
                    let _: Token![=] = input.parse()?;
                    let s: LitStr = input.parse()?;

                    assert_eq!(
                        s.value(),
                        "disabled",
                        "only `dumping = \"disabled\"` is supported"
                    );

                    args.dumping_allowed = false;
                }
                // TODO: call it `crate` like in linkme?
                "elfo" => {
                    let _: Token![=] = input.parse()?;
                    args.crate_ = input.parse()?;
                }
                "not" => {
                    let content;
                    parenthesized!(content in input);
                    args.not = content
                        .parse_terminated::<_, Token![,]>(Ident::parse)?
                        .iter()
                        .map(|ident| ident.to_string())
                        .collect();
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

fn gen_derive_attr(blacklist: &[String], name: &str, path: TokenStream2) -> TokenStream2 {
    let tokens = if blacklist.iter().all(|x| x != name) {
        quote! { #[derive(#path)] }
    } else {
        quote! {}
    };

    tokens.into_token_stream()
}

// TODO: add `T: Debug` for type arguments.
fn gen_impl_debug(input: &DeriveInput) -> TokenStream2 {
    let name = &input.ident;
    let field = match &input.data {
        Data::Struct(data) => {
            assert_eq!(
                data.fields.len(),
                1,
                "`transparent` is applicable only for structs with one field"
            );
            data.fields.iter().next().unwrap()
        }
        Data::Enum(_) => panic!("`transparent` is applicable for structs only"),
        Data::Union(_) => panic!("`transparent` is applicable for structs only"),
    };

    let propagate_fmt = if let Some(ident) = field.ident.as_ref() {
        quote! { self.#ident.fmt(f) }
    } else {
        quote! { self.0.fmt(f) }
    };

    quote! {
        impl ::std::fmt::Debug for #name {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                #propagate_fmt
            }
        }
    }
}

pub fn message_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as MessageArgs);

    // TODO: what about parsing into something cheaper?
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let mod_name = format_ident!("_elfo_{}", name);
    let ltid = gen_ltid();
    let serde_crate = format!("{}::_priv::serde", args.crate_.to_token_stream());
    let crate_ = args.crate_;
    let internal = quote![#crate_::_priv];

    let protocol = std::env::var("CARGO_PKG_NAME").expect("building without cargo?");

    let name_str = args
        .name
        .as_ref()
        .map(LitStr::value)
        .unwrap_or_else(|| input.ident.to_string());

    let impl_request = if let Some(ret) = &args.ret {
        assert!(!args.part, "`part` and `ret` attributes are incompatible");

        quote! {
            impl #crate_::Request for #name {
                type Response = #ret;
                type Wrapper = #mod_name::Wrapper;
            }
        }
    } else {
        quote! {}
    };

    let ret_wrapper = if let Some(ret) = &args.ret {
        let wrapper_name_str = format!("{}::Response", name_str);

        quote! {
            // `message` is imported in the module.
            #[message(not(Debug), name = #wrapper_name_str, elfo = #crate_)]
            pub struct Wrapper(#ret);

            impl fmt::Debug for Wrapper {
                #[inline]
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    self.0.fmt(f)
                }
            }

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

    let derive_debug = if !args.transparent {
        gen_derive_attr(&args.not, "Debug", quote![Debug])
    } else {
        Default::default()
    };
    let derive_clone = gen_derive_attr(&args.not, "Clone", quote![Clone]);
    let derive_serialize =
        gen_derive_attr(&args.not, "Serialize", quote![#internal::serde::Serialize]);
    let derive_deserialize = gen_derive_attr(
        &args.not,
        "Deserialize",
        quote![#internal::serde::Deserialize],
    );

    let serde_crate_attr = if !derive_serialize.is_empty() || !derive_deserialize.is_empty() {
        quote! { #[serde(crate = #serde_crate)] }
    } else {
        quote! {}
    };

    let serde_transparent_attr = if args.transparent {
        quote! { #[serde(transparent)] }
    } else {
        quote! {}
    };

    let impl_debug = if args.transparent && args.not.iter().all(|x| x != "Debug") {
        gen_impl_debug(&input)
    } else {
        quote! {}
    };

    // TODO: pass to `Wrapper`.
    let dumping_allowed = args.dumping_allowed;

    let impl_message = if !args.part {
        quote! {
            impl #crate_::Message for #name {
                const _LTID: #internal::LocalTypeId = #ltid;
                const PROTOCOL: &'static str = #protocol;
                const NAME: &'static str = #name_str;
                const LABELS: &'static [#internal::metrics::Label] = &[
                    #internal::metrics::Label::from_static_parts("message", Self::NAME),
                    #internal::metrics::Label::from_static_parts("protocol", Self::PROTOCOL),
                ];
                const DUMPING_ALLOWED: bool = #dumping_allowed;

                #[inline(always)]
                fn _touch(&self) {
                    #mod_name::touch();
                }
            }

            #[doc(hidden)]
            #[allow(non_snake_case)]
            mod #mod_name {
                use super::*;
                use #crate_::Message;

                use std::fmt;

                use #internal::{
                    MESSAGE_LIST, MessageVTable, AnyMessage,
                    smallbox::smallbox, linkme
                };

                #ret_wrapper

                fn cast_ref(message: &AnyMessage) -> &#name {
                    message.downcast_ref::<#name>().expect("invalid vtable")
                }

                fn clone(message: &AnyMessage) -> AnyMessage {
                    AnyMessage::new(Clone::clone(cast_ref(message)))
                }

                fn debug(message: &AnyMessage, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    fmt::Debug::fmt(cast_ref(message), f)
                }

                fn erase(message: &AnyMessage) -> #crate_::dumping::ErasedMessage {
                    smallbox!(Clone::clone(cast_ref(message)))
                }

                #[linkme::distributed_slice(MESSAGE_LIST)]
                #[linkme(crate = #internal::linkme)]
                static VTABLE: MessageVTable = MessageVTable {
                    ltid: #name::_LTID,
                    name: #name::NAME,
                    protocol: #name::PROTOCOL,
                    labels: #name::LABELS,
                    dumping_allowed: #name::DUMPING_ALLOWED,
                    clone,
                    debug,
                    erase,
                };

                // See [rust#47384](https://github.com/rust-lang/rust/issues/47384).
                #[doc(hidden)]
                #[inline(never)]
                pub fn touch() {}
            }
        }
    } else {
        quote! {}
    };

    TokenStream::from(quote! {
        #derive_debug
        #derive_clone
        #derive_serialize
        #derive_deserialize
        #serde_crate_attr
        #serde_transparent_attr
        #input
        #impl_message
        #impl_request
        #impl_debug
    })
}
