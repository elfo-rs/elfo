use std::{env, sync::Mutex};

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::{
    parenthesized,
    parse::{Error as ParseError, Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
    Data, DeriveInput, Error, Ident, LitStr, Path, Token, Type,
};

use crate::errors::emit_error;

#[derive(Debug)]
struct MessageArgs {
    name: Option<LitStr>,
    protocol: Option<LitStr>,
    ret: Option<Type>,
    part: bool,
    transparent: bool,
    dumping_allowed: bool,
    crate_: Option<Path>,
    not: Vec<String>,
}

impl Parse for MessageArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self, ParseError> {
        let mut args = MessageArgs {
            ret: None,
            name: None,
            protocol: None,
            part: false,
            transparent: false,
            dumping_allowed: true,
            crate_: None,
            not: Vec::new(),
        };

        // `#[message]`
        // `#[message(name = "N")]`
        // `#[message(protocol = "P")]`
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
                "protocol" => {
                    let _: Token![=] = input.parse()?;
                    args.protocol = Some(input.parse()?);
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

                    if s.value() == "disabled" {
                        args.dumping_allowed = false;
                    } else {
                        return Err(input.error("only `dumping = \"disabled\"` is supported"));
                    }
                }
                // TODO: call it `crate` like in linkme?
                "elfo" => {
                    let _: Token![=] = input.parse()?;
                    args.crate_ = Some(input.parse()?);
                }
                "not" => {
                    let content;
                    parenthesized!(content in input);
                    args.not = content
                        .parse_terminated(Ident::parse, Token![,])?
                        .iter()
                        .map(|ident| ident.to_string())
                        .collect();
                }
                _ => return Err(input.error("unknown attribute")),
            }

            if !input.is_empty() {
                let _: Token![,] = input.parse()?;
            }
        }

        Ok(args)
    }
}

fn gen_derive_attr(blacklist: &[String], name: &str, path: TokenStream2) -> TokenStream2 {
    let tokens = if blacklist.iter().all(|x| x != name) {
        quote! { #[derive(#path)] }
    } else {
        quote! {}
    };

    tokens.into_token_stream()
}

fn gen_impl_debug(input: &DeriveInput) -> TokenStream2 {
    let name = &input.ident;
    let field = match &input.data {
        Data::Struct(data) if data.fields.len() == 1 => Some(data.fields.iter().next().unwrap()),
        _ => None,
    };

    let Some(field) = field else {
        emit_error!(
            name.span(),
            "`transparent` is applicable only for structs with one field"
        );
        return TokenStream2::new();
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

fn package_name() -> String {
    env::var("CARGO_PKG_NAME").expect("building without cargo is unsupported for now")
}

fn ensure_proto_name_uniqueness(protocol: &str, name: &str) {
    static MESSAGE_TITLES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    // Skip checks for tests in the `elfo` crate.
    // Note: it doesn't affect other `elfo-*` crates.
    if package_name() == "elfo" {
        return;
    }

    let mut set = MESSAGE_TITLES.lock().unwrap();
    if set.iter().any(|(p, n)| p == protocol && n == name) {
        emit_error!(
            name.span(),
            "duplicate message `{name}` in protocol `{protocol}`\n\
             consider renaming, moving to another crate or specifying protocol explicitly",
        );
    }

    set.push((protocol.to_string(), name.to_string()));
}

pub fn message_impl(
    args: TokenStream,
    input: TokenStream,
    default_path_to_elfo: Path,
) -> TokenStream {
    let args = parse_macro_input!(args as MessageArgs);
    let crate_ = args.crate_.unwrap_or(default_path_to_elfo);

    // TODO: what about parsing into something cheaper?
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let serde_crate = format!("{}::_priv::serde", crate_.to_token_stream());
    let internal = quote![#crate_::_priv];

    let name_str = args
        .name
        .as_ref()
        .map(LitStr::value)
        .unwrap_or_else(|| input.ident.to_string());

    let protocol_str = args
        .protocol
        .as_ref()
        .map(LitStr::value)
        .unwrap_or_else(package_name);

    if !args.part {
        ensure_proto_name_uniqueness(&protocol_str, &name_str);
    }

    let ret_wrapper = if let Some(ret) = &args.ret {
        let wrapper_name_str = format!("{name_str}::Response");

        quote! {
            #[message(not(Debug), protocol = #protocol_str, name = #wrapper_name_str, elfo = #crate_)]
            pub struct _elfo_Wrapper(#ret);

            impl ::std::fmt::Debug for _elfo_Wrapper {
                #[inline]
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    self.0.fmt(f)
                }
            }

            impl From<#ret> for _elfo_Wrapper {
                #[inline]
                fn from(inner: #ret) -> Self {
                    _elfo_Wrapper(inner)
                }
            }

            impl From<_elfo_Wrapper> for #ret {
                #[inline]
                fn from(wrapper: _elfo_Wrapper) -> Self {
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

    // TODO: pass to `_elfo_Wrapper`.
    let dumping_allowed = args.dumping_allowed;

    let network_fns = if cfg!(feature = "network") {
        quote! {
            fn write_msgpack(
                message: &#internal::AnyMessage,
                buffer: &mut Vec<u8>,
                limit: usize
            ) -> ::std::result::Result<(), #internal::rmps::encode::Error> {
                #internal::write_msgpack(buffer, limit, cast_ref(message))
            }

            fn read_msgpack(buffer: &[u8]) ->
                ::std::result::Result<#internal::AnyMessage, #internal::rmps::decode::Error>
            {
                #internal::read_msgpack::<#name>(buffer).map(#crate_::Message::upcast)
            }
        }
    } else {
        quote! {}
    };

    let network_fns_ref = if cfg!(feature = "network") {
        quote! { write_msgpack, read_msgpack }
    } else {
        quote! {}
    };

    let impl_message = if !args.part {
        quote! {
            impl #crate_::Message for #name {
                #[inline(always)]
                fn _vtable(&self) -> &'static #internal::MessageVTable {
                    &VTABLE
                }

                #[inline(always)]
                fn _touch(&self) {
                    touch();
                }
            }

            #ret_wrapper

            fn cast_ref(message: &#internal::AnyMessage) -> &#name {
                message.downcast_ref::<#name>().expect("invalid vtable")
            }

            fn clone(message: &#internal::AnyMessage) -> #internal::AnyMessage {
                #crate_::Message::upcast(Clone::clone(cast_ref(message)))
            }

            fn debug(message: &#internal::AnyMessage, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::std::fmt::Debug::fmt(cast_ref(message), f)
            }

            fn erase(message: &#internal::AnyMessage) -> #crate_::dumping::ErasedMessage {
                smallbox!(Clone::clone(cast_ref(message)))
            }

            #network_fns

            #[linkme::distributed_slice(MESSAGE_LIST)]
            #[linkme(crate = linkme)]
            static VTABLE: &'static #internal::MessageVTable = &#internal::MessageVTable {
                name: #name_str,
                protocol: #protocol_str,
                labels: &[
                    #internal::metrics::Label::from_static_parts("message", #name_str),
                    #internal::metrics::Label::from_static_parts("protocol", #protocol_str),
                ],
                dumping_allowed: #dumping_allowed,
                clone,
                debug,
                erase,
                #network_fns_ref
            };

            // See [rust#47384](https://github.com/rust-lang/rust/issues/47384).
            #[doc(hidden)]
            #[inline(never)]
            pub fn touch() {}
        }
    } else {
        quote! {}
    };

    let impl_request = if let Some(ret) = &args.ret {
        if args.part {
            emit_error!(ret.span(), "`part` and `ret` attributes are incompatible");
        }

        quote! {
            impl #crate_::Request for #name {
                type Response = #ret;
                type Wrapper = _elfo_Wrapper;
            }
        }
    } else {
        quote! {}
    };

    let impl_debug = if args.transparent && args.not.iter().all(|x| x != "Debug") {
        gen_impl_debug(&input)
    } else {
        quote! {}
    };

    let errors = crate::errors::into_tokens();

    TokenStream::from(quote! {
        #derive_debug
        #derive_clone
        #derive_serialize
        #derive_deserialize
        #serde_crate_attr
        #serde_transparent_attr
        #input

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #[allow(unreachable_code)] // for `enum Impossible {}`
        const _: () = {
            // Keep this list as minimal as possible to avoid possible collisions with `#name`.
            // Especially avoid `PascalCase`.
            use #internal::{MESSAGE_LIST, smallbox::smallbox, linkme};

            #impl_message
            #impl_request
            #impl_debug

            #errors
        };
    })
}
