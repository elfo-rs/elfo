#![warn(rust_2018_idioms, unreachable_pub)]

pub use expect::{expect_message, expect_request};
pub use proxy::{proxy, Proxy};

#[cfg(feature = "unstable")]
pub use proxy::proxy_with_route;

mod expect;
mod proxy;
