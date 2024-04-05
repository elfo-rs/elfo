use std::cell::RefCell;

use proc_macro2::TokenStream;
use syn::Error;

thread_local! {
    static ERROR: RefCell<Option<Error>> = const { RefCell::new(None) };
}

macro_rules! emit_error {
    ($span:expr, $($tt:tt)*) => {
        crate::errors::emit(syn::Error::new($span, format!($($tt)*)))
    }
}

pub(crate) use emit_error;

pub(crate) fn emit(error: Error) {
    ERROR.with(|combined| {
        let mut combined = combined.borrow_mut();

        if let Some(combined) = &mut *combined {
            combined.combine(error);
        } else {
            *combined = Some(error);
        }
    });
}

pub(crate) fn into_tokens() -> Option<TokenStream> {
    ERROR.with(|e| e.borrow_mut().take().map(|e| e.into_compile_error()))
}
