use proc_macro2::{Span, TokenStream};
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
    /// A message's name override.
    /// If `None`, the message's name is the same as the type's name.
    name: Option<LitStr>,
    /// A message's protocol override.
    /// If `None`, it is a result of `elfo::get_protocol!()`:
    /// * `elfo::set_protocol!()` in the current module
    /// * or, usually, a package name
    protocol: Option<LitStr>,
    /// A message is request with this response type.
    ret: Option<Type>,
    /// Don't implement `Message` or `Request` traits.
    part: bool,
    /// Adds `serde(transparent)` and implement transparent `Debug`.
    transparent: bool,
    /// Specifies the dumping level. `Normal` by default.
    /// Must be a variant of `elfo::dumping::Level`.
    dumping_level: Option<Ident>,
    /// Override the path to the `elfo` crate.
    crate_: Option<Path>,
    /// Don't derive these traits (see `GENERATED_IMPLS`).
    not: Vec<Ident>,
}

/// `elfo::dumping::Level` variants.
const DUMPING_LEVELS: [&str; 4] = ["Normal", "Verbose", "Total", "Never"];
const GENERATED_IMPLS: [&str; 4] = ["Debug", "Clone", "Serialize", "Deserialize"];

impl Parse for MessageArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self, ParseError> {
        let mut args = MessageArgs {
            ret: None,
            name: None,
            protocol: None,
            part: false,
            transparent: false,
            dumping_level: None,
            crate_: None,
            not: Vec::new(),
        };

        while !input.is_empty() {
            let ident: Ident = input.parse()?;

            match ident.to_string().as_str() {
                // name = "Some"
                "name" => {
                    let _: Token![=] = input.parse()?;
                    args.name = Some(input.parse()?);
                }
                // protocol = "some"
                "protocol" => {
                    let _: Token![=] = input.parse()?;
                    args.protocol = Some(input.parse()?);
                }
                // ret = path::to::Type
                "ret" => {
                    let _: Token![=] = input.parse()?;
                    args.ret = Some(input.parse()?);
                }
                // part (boolean)
                "part" => args.part = true,
                // transparent (boolean)
                "transparent" => args.transparent = true,
                // dumping = "Normal"
                // dumping = "Verbose"
                // dumping = "Total"
                // dumping = "Never"
                "dumping" => {
                    let _: Token![=] = input.parse()?;
                    let lit: LitStr = input.parse()?;
                    let s = lit.value();

                    if DUMPING_LEVELS.contains(&s.as_str()) {
                        args.dumping_level = Some(Ident::new(&s, lit.span()));
                    } else {
                        emit_error!(
                            s.span(),
                            "allowed `dumping` values: {}",
                            DUMPING_LEVELS.join(", ")
                        );
                    }
                }
                // elfo = path::to::elfo::crate
                // TODO: call it `crate` like in linkme?
                "elfo" => {
                    let _: Token![=] = input.parse()?;
                    args.crate_ = Some(input.parse()?);
                }
                // not(Debug, Clone)
                "not" => {
                    let content;
                    parenthesized!(content in input);

                    args.not = content
                        .parse_terminated(Ident::parse, Token![,])?
                        .into_iter()
                        .filter(|ident| {
                            let is_allowed =
                                GENERATED_IMPLS.into_iter().any(|allowed| ident == allowed);

                            if !is_allowed {
                                emit_error!(
                                    ident.span(),
                                    "allowed `not` values: {}",
                                    GENERATED_IMPLS.join(", ")
                                );
                            }

                            is_allowed
                        })
                        .collect();
                }
                _ => return Err(input.error("unknown attribute")),
            }

            if !input.is_empty() {
                let _: Token![,] = input.parse()?;
            }
        }

        args.validate();

        Ok(args)
    }
}

impl MessageArgs {
    fn validate(&self) {
        if self.part {
            fn incompatible_with_part(spanned: &Option<impl Spanned>, name: &str) {
                if let Some(span) = spanned.as_ref().map(|s| s.span()) {
                    emit_error!(span, "`part` and `{name}` attributes are incompatible");
                }
            }

            incompatible_with_part(&self.ret, "ret");
            incompatible_with_part(&self.name, "name");
            incompatible_with_part(&self.protocol, "protocol");
            incompatible_with_part(&self.dumping_level, "dumping");
        }
    }

    fn should_impl(&self, name: &str) -> bool {
        self.not.iter().all(|not| not != name)
    }
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
    let crate_ = args.crate_.as_ref().unwrap_or(&default_path_to_elfo);

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
        (!args.transparent && args.should_impl("Debug")).then(|| quote! { #[derive(Debug)] });
    let derive_clone = args
        .should_impl("Clone")
        .then(|| quote! { #[derive(Clone)] });
    let derive_serialize = args
        .should_impl("Serialize")
        .then(|| quote! { #[derive(#internal::serde::Serialize)] });
    let derive_deserialize = args
        .should_impl("Deserialize")
        .then(|| quote! { #[derive(#internal::serde::Deserialize)] });

    let serde_crate_attr = (derive_serialize.is_some() || derive_deserialize.is_some())
        .then(|| quote! { #[serde(crate = #serde_crate)] });

    let serde_transparent_attr = args.transparent.then(|| quote! { #[serde(transparent)] });

    let dumping_level = args
        .dumping_level
        .clone()
        .unwrap_or_else(|| Ident::new("Normal", Span::call_site()));

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
                #crate_::dumping::Level::#dumping_level,
            );
        }
    });

    let impl_request = args.ret.as_ref().map(|ret| {
        let wrapper_name_str = format!("{name_str}::Response");
        let protocol = args.protocol.as_ref().map(|p| quote! { protocol = #p, });
        let dumping_level_str = dumping_level.to_string();

        quote! {
            #[automatically_derived]
            impl #crate_::Request for #name {
                type Response = #ret;
                type Wrapper = ElfoResponseWrapper;
            }

            #[message(
                #protocol name = #wrapper_name_str,
                dumping = #dumping_level_str,
                not(Debug), elfo = #crate_
            )]
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
        (args.transparent && args.should_impl("Debug")).then(|| gen_impl_debug(&input));

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
