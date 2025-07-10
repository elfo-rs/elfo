use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    parenthesized,
    parse::{Error as ParseError, Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
    Data, DeriveInput, Ident, LitStr, Path, Token, Type,
};

use crate::errors::emit_error;

#[derive(Debug)]
struct MessageArgs {
    name: Option<LitStr>,
    protocol: Option<LitStr>,
    ret: Option<Type>,
    part: bool,
    transparent: bool,
    dumping_allowed: Option<bool>,
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
            dumping_allowed: None,
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
                        args.dumping_allowed = Some(false);
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

impl MessageArgs {
    fn validate(&self) {
        if self.part {
            fn incompatible(spanned: &Option<impl Spanned>, name: &str) {
                if let Some(span) = spanned.as_ref().map(|s| s.span()) {
                    emit_error!(span, "`part` and `{name}` attributes are incompatible");
                }
            }

            incompatible(&self.ret, "ret");
            incompatible(&self.name, "name");
            incompatible(&self.protocol, "protocol");
            incompatible(&self.dumping_allowed, "dumping_allowed");
        }
    }
}

fn gen_derive_attr(blacklist: &[String], name: &str, path: TokenStream) -> TokenStream {
    blacklist
        .iter()
        .all(|x| x != name)
        .then(|| quote! { #[derive(#path)] })
        .into_token_stream()
}

fn gen_impl_debug(input: &DeriveInput) -> TokenStream {
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
        return TokenStream::new();
    };

    let propagate_fmt = if let Some(ident) = field.ident.as_ref() {
        quote! { ::std::fmt::Debug::fmt(&self.#ident, f) }
    } else {
        quote! { ::std::fmt::Debug::fmt(&self.0, f) }
    };

    quote! {
        #[automatically_derived]
        impl ::std::fmt::Debug for #name {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                #propagate_fmt
            }
        }
    }
}

/// Implementation of the `#[message]` macro.
pub fn message_impl(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
    default_path_to_elfo: Path,
) -> proc_macro::TokenStream {
    let args = parse_macro_input!(args as MessageArgs);
    args.validate();

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

    let derive_debug =
        (!args.transparent).then(|| gen_derive_attr(&args.not, "Debug", quote![Debug]));
    let derive_clone = gen_derive_attr(&args.not, "Clone", quote![Clone]);
    let derive_serialize =
        gen_derive_attr(&args.not, "Serialize", quote![#internal::serde::Serialize]);
    let derive_deserialize = gen_derive_attr(
        &args.not,
        "Deserialize",
        quote![#internal::serde::Deserialize],
    );

    let serde_crate_attr = (!derive_serialize.is_empty() || !derive_deserialize.is_empty())
        .then(|| quote! { #[serde(crate = #serde_crate)] });

    let serde_transparent_attr = args.transparent.then(|| quote! { #[serde(transparent)] });

    // TODO: pass to `ElfoResponseWrapper`.
    let dumping_allowed = args.dumping_allowed.unwrap_or(true);

    let protocol = if let Some(protocol) = &args.protocol {
        quote! { #protocol }
    } else {
        quote! { #crate_::get_protocol!() }
    };

    let impl_message = (!args.part).then(|| {
        quote! {
            #[automatically_derived]
            impl #crate_::Message for #name {
                #[inline(always)]
                fn _type_id() -> #internal::MessageTypeId {
                    #internal::MessageTypeId::new(VTABLE)
                }

                #[inline(always)]
                fn _vtable(&self) -> &'static #internal::MessageVTable {
                    VTABLE
                }
            }

            #[#internal::linkme::distributed_slice(#internal::MESSAGE_VTABLES_LIST)]
            #[linkme(crate = #internal::linkme)]
            static VTABLE: &#internal::MessageVTable = &#internal::MessageVTable::new::<#name>(
                #name_str,
                #protocol,
                #dumping_allowed
            );
        }
    });

    let impl_request = args.ret.as_ref().map(|ret| {
        let wrapper_name_str = format!("{name_str}::Response");
        let protocol = args.protocol.as_ref().map(|p| quote! { protocol = #p, });

        quote! {
            #[automatically_derived]
            impl #crate_::Request for #name {
                type Response = #ret;
                type Wrapper = ElfoResponseWrapper;
            }

            #[message(not(Debug), #protocol name = #wrapper_name_str, elfo = #crate_)]
            pub struct ElfoResponseWrapper(#ret);

            #[automatically_derived]
            impl ::std::fmt::Debug for ElfoResponseWrapper {
                #[inline]
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    ::std::fmt::Debug::fmt(&self.0, f)
                }
            }

            #[automatically_derived]
            impl From<#ret> for ElfoResponseWrapper {
                #[inline]
                fn from(inner: #ret) -> Self {
                    ElfoResponseWrapper(inner)
                }
            }

            #[automatically_derived]
            impl From<ElfoResponseWrapper> for #ret {
                #[inline]
                fn from(wrapper: ElfoResponseWrapper) -> Self {
                    wrapper.0
                }
            }
        }
    });

    let impl_debug =
        (args.transparent && args.not.iter().all(|x| x != "Debug")).then(|| gen_impl_debug(&input));

    // Don't add `use` statements here to avoid possible collisions with user code.
    let expanded = quote! {
        #derive_debug
        #derive_clone
        #derive_serialize
        #derive_deserialize
        #serde_crate_attr
        #serde_transparent_attr
        #input

        #[doc(hidden)]
        #[allow(unreachable_code)] // for `enum Impossible {}`
        const _: () = {
            #impl_message
            #impl_request
            #impl_debug
        };
    };

    // Errors must be checked after expansion, otherwise some errors can be lost.
    if let Some(errors) = crate::errors::into_tokens() {
        quote! { #expanded #errors }.into()
    } else {
        expanded.into()
    }
}
