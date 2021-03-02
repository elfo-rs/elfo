use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Arm, ExprMatch, Pat, PatIdent, Path};

// TODO: use `proc-macro-error` instead of `panic!`.
// TODO: use `proc-macro-crate`?

struct MessageGroup {
    kind: GroupKind,
    arms: Vec<Arm>,
}

#[derive(Debug, PartialEq)]
enum GroupKind {
    // `msg @ Msg(..) => ...`
    Regular(Path),
    // `(msg @ Msg(..), token) => ...`
    Request(Path),
    // `_ =>`
    // `msg =>`
    Wild,
}

fn is_valid_token_ident(ident: &PatIdent) -> bool {
    !ident.ident.to_string().starts_with('_')
}

fn extract_kind(pat: &Pat, is_top_level: bool) -> Result<GroupKind, &'static str> {
    match pat {
        Pat::Box(_) => Err("box patterns are forbidden"),
        Pat::Ident(pat) => pat
            .subpat
            .as_ref()
            .map(|sp| extract_kind(&sp.1, false))
            .unwrap_or(Ok(GroupKind::Wild)),
        Pat::Lit(_) => Err("literal patterns are forbidden"),
        Pat::Macro(_) => Err("macros in pattern position are forbidden"),
        Pat::Or(pat) => pat
            .cases
            .iter()
            .find_map(|pat| extract_kind(pat, false).ok())
            .ok_or("cannot determine the message's type"),
        Pat::Path(pat) => Ok(GroupKind::Regular(pat.path.clone())),
        Pat::Range(_) => Err("range patterns are forbidden"),
        Pat::Reference(pat) => extract_kind(&pat.pat, false),
        Pat::Rest(_) => Err("rest patterns are forbidden"),
        Pat::Slice(_) => Err("slice patterns are forbidden"),
        Pat::Struct(pat) => Ok(GroupKind::Regular(pat.path.clone())),
        Pat::Tuple(pat) if is_top_level => {
            assert_eq!(pat.elems.len(), 2, "invalid request pattern");

            match pat.elems.last().unwrap() {
                Pat::Ident(pat) if is_valid_token_ident(pat) => {}
                _ => panic!("the token must be used"),
            }

            match extract_kind(pat.elems.first().unwrap(), false)? {
                GroupKind::Regular(path) => Ok(GroupKind::Request(path)),
                _ => Err("cannot determine the request's type"),
            }
        }
        Pat::Tuple(_) => Err("tuple patterns are forbidden"),
        Pat::TupleStruct(pat) => Ok(GroupKind::Regular(pat.path.clone())),
        Pat::Type(_) => Err("type ascription patterns are forbidden"),
        Pat::Wild(_) => Ok(GroupKind::Wild),
        _ => Err("unknown tokens"),
    }
}

pub fn msg_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ExprMatch);
    let mut groups = Vec::<MessageGroup>::with_capacity(input.arms.len());

    for arm in input.arms.into_iter() {
        let kind = extract_kind(&arm.pat, true).expect("invalid pattern");

        match groups.iter_mut().find(|group| group.kind == kind) {
            Some(group) => group.arms.push(arm),
            None => groups.push(MessageGroup {
                kind,
                arms: vec![arm],
            }),
        }
    }

    let envelope_ident = quote! { __elfo_envelope };
    let message_ident = quote! { __elfo_message };
    let tx_ident = quote! { __elfo_tx };
    let token_ident = quote! { __elfo_token };

    let groups = groups
        .iter()
        .map(|group| match (&group.kind, &group.arms[..]) {
            (GroupKind::Regular(path), arms) => quote! {
                else if #envelope_ident.is::<#path>() {
                    // TODO: replace with `static_assertions`.
                    trait Forbidden<A, E> { fn test(_: &E) {} }
                    impl<E, M> Forbidden<(), E> for M {}
                    struct Invalid;
                    impl<E: EnvelopeOwned, M: elfo::Request> Forbidden<Invalid, E> for M {}
                    let _ = <#path as Forbidden<_, _>>::test(&#envelope_ident);
                    // -----

                    let #message_ident = #envelope_ident.unpack_regular();
                    match #message_ident.downcast2::<#path>() {
                        #(#arms)*
                    }
                }
            },
            (GroupKind::Request(path), arms) => quote! {
                else if #envelope_ident.is::<#path>() {
                    static_assertions::assert_impl_all!(#path: elfo::Request);
                    let (#message_ident, #tx_ident) = #envelope_ident.unpack_request();
                    let #token_ident: elfo::ReplyToken<#path> = elfo::ReplyToken::from_sender(#tx_ident);
                    match (#message_ident.downcast2::<#path>(), #token_ident) {
                        #(#arms)*
                    }
                }
            },
            (GroupKind::Wild, &[ref arm]) => quote! {
                else {
                    match #envelope_ident { #arm }
                }
            },
            (GroupKind::Wild, _) => panic!("too many default branches"),
        })
        .collect::<Vec<_>>();

    let match_expr = input.expr;

    // TODO: propagate `input.attrs`?
    let expanded = quote! {{
        use elfo::_priv::*;
        let #envelope_ident = #match_expr;
        if false { unreachable!(); }
        #(#groups)*
    }};

    TokenStream::from(expanded)
}
