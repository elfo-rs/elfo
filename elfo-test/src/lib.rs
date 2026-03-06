//! Utils for unit testing actors.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub use proxy::{proxy, Proxy};
pub use utils::{extract_message, extract_request};

#[cfg(feature = "unstable")]
pub use proxy::proxy_with_route;

mod proxy;
mod utils;
