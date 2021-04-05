use std::{char, collections::HashMap};

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Arm, ExprMatch, Ident, Pat, PatIdent, Path};

// TODO: use `proc-macro-error` instead of `panic!`.
// TODO: use `proc-macro-crate`?

struct MessageGroup {
    kind: GroupKind,
    arms: Vec<Arm>,
}

#[derive(Debug, Hash, PartialEq, Eq)]
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

fn is_type_ident(ident: &Ident) -> bool {
    ident
        .to_string()
        .chars()
        .next()
        .map_or(false, char::is_uppercase)
}

fn extract_kind(pat: &Pat) -> Result<GroupKind, &'static str> {
    match pat {
        Pat::Box(_) => Err("box patterns are forbidden"),
        Pat::Ident(pat) => match pat.subpat.as_ref() {
            Some(sp) => extract_kind(&sp.1),
            None if is_type_ident(&pat.ident) => {
                Ok(GroupKind::Regular(Path::from(pat.ident.clone())))
            }
            None => Ok(GroupKind::Wild),
        },
        Pat::Lit(_) => Err("literal patterns are forbidden"),
        Pat::Macro(_) => Err("macros in pattern position are forbidden"),
        Pat::Or(pat) => pat
            .cases
            .iter()
            .find_map(|pat| extract_kind(pat).ok())
            .ok_or("cannot determine the message's type"),
        Pat::Path(pat) => Ok(GroupKind::Regular(pat.path.clone())),
        Pat::Range(_) => Err("range patterns are forbidden"),
        Pat::Reference(pat) => extract_kind(&pat.pat),
        Pat::Rest(_) => Err("rest patterns are forbidden"),
        Pat::Slice(_) => Err("slice patterns are forbidden"),
        Pat::Struct(pat) => Ok(GroupKind::Regular(pat.path.clone())),
        Pat::Tuple(pat) => {
            assert_eq!(pat.elems.len(), 2, "invalid request pattern");

            match pat.elems.last().unwrap() {
                Pat::Ident(pat) if is_valid_token_ident(pat) => {}
                _ => panic!("the token must be used"),
            }

            match extract_kind(pat.elems.first().unwrap())? {
                GroupKind::Regular(path) => Ok(GroupKind::Request(path)),
                _ => Err("cannot determine the request's type"),
            }
        }
        Pat::TupleStruct(pat) => Ok(GroupKind::Regular(pat.path.clone())),
        Pat::Type(_) => Err("type ascription patterns are forbidden"),
        Pat::Wild(_) => Ok(GroupKind::Wild),
        _ => Err("unknown tokens"),
    }
}

fn add_groups(groups: &mut Vec<MessageGroup>, arm: Arm) -> Result<(), &'static str> {
    let mut add = |kind, arm: Arm| {
        // println!("group {:?} {:#?}", kind, arm.pat);
        match groups.iter_mut().find(|common| common.kind == kind) {
            Some(common) => common.arms.push(arm),
            None => groups.push(MessageGroup {
                kind,
                arms: vec![arm],
            }),
        }
    };

    if let Pat::Or(pat) = &arm.pat {
        let mut map = HashMap::new();

        for pat in &pat.cases {
            let kind = extract_kind(pat)?;
            let new_arm = map.entry(kind).or_insert_with(|| {
                let mut arm = arm.clone();
                if let Pat::Or(pat) = &mut arm.pat {
                    pat.cases.clear();
                }
                arm
            });

            if let Pat::Or(new_pat) = &mut new_arm.pat {
                new_pat.cases.push(pat.clone());
            }
        }

        for (kind, arm) in map {
            add(kind, arm);
        }
    } else {
        add(extract_kind(&arm.pat)?, arm);
    }

    Ok(())
}

pub fn msg_impl(input: TokenStream, path_to_elfo: Path) -> TokenStream {
    let input = parse_macro_input!(input as ExprMatch);
    let mut groups = Vec::<MessageGroup>::with_capacity(input.arms.len());
    let crate_ = path_to_elfo;

    for arm in input.arms.into_iter() {
        add_groups(&mut groups, arm).expect("invalid pattern");
    }

    let envelope_ident = quote! { _elfo_envelope };

    let groups = groups
        .iter()
        .map(|group| match (&group.kind, &group.arms[..]) {
            (GroupKind::Regular(path), arms) => quote! {
                else if #envelope_ident.is::<#path>() {
                    // TODO: replace with `static_assertions`.
                    trait Forbidden<A, E> { fn test(_: &E) {} }
                    impl<E, M> Forbidden<(), E> for M {}
                    struct Invalid;
                    impl<E: EnvelopeOwned, M: #crate_::Request> Forbidden<Invalid, E> for M {}
                    let _ = <#path as Forbidden<_, _>>::test(&#envelope_ident);
                    // -----

                    let message = #envelope_ident.unpack_regular();
                    match message.downcast2::<#path>() {
                        #(#arms)*
                    }
                }
            },
            (GroupKind::Request(path), arms) => quote! {
                else if #envelope_ident.is::<#path>() {
                    assert_impl_all!(#path: #crate_::Request);
                    let (message, token) = #envelope_ident.unpack_request::<#path>();
                    match (message.downcast2::<#path>(), token) {
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
        });

    let match_expr = input.expr;

    // TODO: propagate `input.attrs`?
    let expanded = quote! {{
        use #crate_::_priv::*;
        let #envelope_ident = #match_expr;
        if false { unreachable!(); }
        #(#groups)*
    }};

    TokenStream::from(expanded)
}
