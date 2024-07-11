use std::{
    any::Any,
    future::Future,
    panic::{self, AssertUnwindSafe},
};

use futures::FutureExt;

pub(crate) fn sync_catch<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(panic_to_string)
}

pub(crate) async fn catch<R>(f: impl Future<Output = R>) -> Result<R, String> {
    AssertUnwindSafe(f)
        .catch_unwind()
        .await
        .map_err(panic_to_string)
}

fn panic_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("panic: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("panic: {message}")
    } else {
        "panic: <unsupported payload>".to_string()
    }
}
