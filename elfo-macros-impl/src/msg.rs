use std::{char, collections::HashMap};

use quote::{quote, quote_spanned};
use syn::{
    parse_macro_input, spanned::Spanned, Arm, ExprMatch, Ident, Pat, PatIdent, PatWild, Path, Token,
};

use crate::errors::emit_error;

#[derive(Debug)]
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

fn extract_path_to_type(path: &Path) -> Path {
    let mut ident_rev_it = path.segments.iter().rev();

    // Handle enum variants:
    // `some::Enum::Variant`
    //        ^- must be uppercased
    //
    // Yep, it's crazy, but it seems to be a good assumption for now.
    if let Some(prev) = ident_rev_it.nth(1) {
        if is_type_ident(&prev.ident) {
            let mut path = path.clone();
            path.segments.pop().unwrap();

            // Convert `Pair::Punctuated` to `Pair::End`.
            let (last, _) = path.segments.pop().unwrap().into_tuple();
            path.segments.push(last);
            return path;
        }
    }

    path.clone()
}

fn extract_kind(pat: &Pat) -> Result<GroupKind, &'static str> {
    match pat {
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
        Pat::Paren(_) => Err("parenthesized patterns are forbidden"),
        Pat::Path(pat) => Ok(GroupKind::Regular(extract_path_to_type(&pat.path))),
        Pat::Range(_) => Err("range patterns are forbidden"),
        Pat::Reference(pat) => extract_kind(&pat.pat),
        Pat::Rest(_) => Err("rest patterns are forbidden"),
        Pat::Slice(_) => Err("slice patterns are forbidden"),
        Pat::Struct(pat) => Ok(GroupKind::Regular(extract_path_to_type(&pat.path))),
        Pat::Tuple(pat) => {
            if pat.elems.len() != 2 {
                return Err("invalid request pattern");
            }

            match pat.elems.last().unwrap() {
                Pat::Ident(pat) => {
                    if !is_valid_token_ident(pat) {
                        emit_error!(
                            pat.span(),
                            "the token must be used, or call `drop(_)` explicitly"
                        )
                    }
                }
                _ => return Err("token must be identifier"),
            }

            match extract_kind(pat.elems.first().unwrap())? {
                GroupKind::Regular(path) => Ok(GroupKind::Request(path)),
                _ => Err("cannot determine the request's type"),
            }
        }
        Pat::TupleStruct(pat) => Ok(GroupKind::Regular(extract_path_to_type(&pat.path))),
        Pat::Type(_) => Err("type ascription patterns are forbidden"),
        Pat::Wild(_) => Ok(GroupKind::Wild),
        _ => Err("unknown tokens"),
    }
}

fn is_likely_type(pat: &Pat) -> bool {
    match pat {
        Pat::Ident(i) if i.subpat.is_none() && is_type_ident(&i.ident) => true,
        Pat::Path(p) if extract_path_to_type(&p.path) == p.path => true,
        _ => false,
    }
}

/// Detects `a @ A` and `a @ some::A` patterns.
fn is_binding_with_type(ident: &PatIdent) -> bool {
    ident
        .subpat
        .as_ref()
        .map_or(false, |sp| is_likely_type(&sp.1))
}

fn refine_pat(pat: &mut Pat) {
    match pat {
        // `e @ Enum`
        // `s @ Struct` (~ `s @ Struct { .. }`)
        Pat::Ident(ident) if is_binding_with_type(ident) => {
            ident.subpat = None;
        }
        // `(e @ SomeType, token)`
        // `(SomeType, token)`
        Pat::Tuple(pat) => {
            // It's ok to use `assert_eq!` here because it must be already checked.
            assert_eq!(pat.elems.len(), 2, "invalid request pattern");

            match pat.elems.first_mut() {
                Some(Pat::Ident(ident)) if is_binding_with_type(ident) => {
                    ident.subpat = None;
                }
                Some(pat) if is_likely_type(pat) => {
                    *pat = Pat::Wild(PatWild {
                        attrs: Vec::new(),
                        underscore_token: Token![_](pat.span()),
                    });
                }
                _ => {}
            }
        }
        // `SomeType => ...`
        pat if is_likely_type(pat) => {
            *pat = Pat::Wild(PatWild {
                attrs: Vec::new(),
                underscore_token: Token![_](pat.span()),
            });
        }
        _ => {}
    };
}

fn add_groups(groups: &mut Vec<MessageGroup>, mut arm: Arm) {
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
            let kind = match extract_kind(pat) {
                Ok(kind) => kind,
                Err(err) => {
                    emit_error!(pat.span(), "{err}");
                    continue;
                }
            };
            let new_arm = map.entry(kind).or_insert_with(|| {
                let mut arm = arm.clone();
                if let Pat::Or(pat) = &mut arm.pat {
                    pat.cases.clear();
                }
                arm
            });

            if let Pat::Or(new_pat) = &mut new_arm.pat {
                let mut old_pat = pat.clone();
                refine_pat(&mut old_pat);
                new_pat.cases.push(old_pat);
            }
        }

        for (kind, arm) in map {
            add(kind, arm);
        }
    } else {
        let kind = match extract_kind(&arm.pat) {
            Ok(kind) => kind,
            Err(err) => return emit_error!(arm.pat.span(), "{err}"),
        };
        refine_pat(&mut arm.pat);
        add(kind, arm);
    }
}

pub fn msg_impl(input: proc_macro::TokenStream, path_to_elfo: Path) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as ExprMatch);
    let mut groups = Vec::<MessageGroup>::with_capacity(input.arms.len());
    let crate_ = path_to_elfo;
    let internal = quote![#crate_::_priv];

    for arm in input.arms.into_iter() {
        add_groups(&mut groups, arm);
    }

    let envelope_ident = quote! { _elfo_envelope };
    let type_id_ident = quote! { _type_id_envelope };

    // println!(">>> HERE {:#?}", groups);

    let groups = groups
        .iter()
        .map(|group| match (&group.kind, &group.arms[..]) {
            // Specify the span for better error localization:
            // - used the regular syntax while the request one is expected
            // - unexhaustive match
            (GroupKind::Regular(path), arms) => quote_spanned! { path.span()=>
                else if #type_id_ident == std::any::TypeId::of::<#path>() {
                    // Ensure it's not a request, or a request but only in a borrowed context.
                    // We cannot use `static_assertions` here because it wraps the check into
                    // a closure that forbids us to use generic `msg!`: (`msg!(match e { M => .. })`).
                    {
                        trait MustBeRegularNotRequest<A, E> { fn test(_: &E) {} }
                        impl<E, M> MustBeRegularNotRequest<(), E> for M {}
                        struct Invalid;
                        impl<E: #internal::EnvelopeOwned, M: #crate_::Request>
                            MustBeRegularNotRequest<Invalid, E> for M {}
                        <#path as MustBeRegularNotRequest<_, _>>::test(&#envelope_ident)
                    }

                    #[allow(unknown_lints, clippy::blocks_in_conditions)]
                    match {
                        // Support both owned and borrowed contexts, relying on the type inference.
                        #[allow(unused_imports)]
                        use #internal::{
                            EnvelopeOwned as _, EnvelopeBorrowed as _,
                            AnyMessageOwned as _, AnyMessageBorrowed as _,
                        };
                        #envelope_ident.unpack_regular().downcast2::<#path>()
                    } {
                        #(#arms)*
                    }
                }
            },
            (GroupKind::Request(path), arms) => quote_spanned! { path.span()=>
                else if #type_id_ident == std::any::TypeId::of::<#path>() {
                    // Ensure it's a request. We cannot use `static_assertions` here
                    // because it wraps the check into a closure that forbids us to
                    // use generic `msg!`: (`msg!(match e { (R, token) => .. })`).
                    {
                        fn must_be_request<R: #crate_::Request>() {}
                        must_be_request::<#path>();
                    }

                    #[allow(unknown_lints, clippy::blocks_in_conditions)]
                    match {
                        // Only the owned context is supported.
                        #[allow(unused_imports)]
                        use #internal::{EnvelopeOwned as _, AnyMessageOwned as _};
                        let (message, token) = #envelope_ident.unpack_request();
                        (message.downcast2::<#path>(), token.into_received::<#path>())
                    } {
                        #(#arms)*
                    }
                }
            },
            (GroupKind::Wild, arms) => {
                let mut arms_iter = arms.iter();
                let arm = arms_iter.next().unwrap();

                let expanded = quote! {
                    else {
                        match #envelope_ident { #arm }
                    }
                };

                for arm in arms_iter {
                    emit_error!(arm.pat.span(), "this branch will never be matched");
                }

                expanded
            }
        });

    let match_expr = input.expr;

    // TODO: propagate `input.attrs`?
    let expanded = quote! {{
        let #envelope_ident = #match_expr;
        let #type_id_ident = #envelope_ident.type_id();
        #[allow(clippy::suspicious_else_formatting)]
        if false { unreachable!(); }
        #(#groups)*
    }};

    // Errors must be checked after expansion, otherwise some errors can be lost.
    if let Some(errors) = crate::errors::into_tokens() {
        quote! {{ #errors #expanded }}.into()
    } else {
        expanded.into()
    }
}
