#![warn(rust_2018_idioms, unreachable_pub)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub use elfo_core::*;
pub use elfo_macros::{message, msg};

#[cfg(feature = "elfo-configurer")]
#[cfg_attr(docsrs, doc(cfg(feature = "full")))]
pub use elfo_configurer as configurer;
#[cfg(feature = "elfo-dumper")]
#[cfg_attr(docsrs, doc(cfg(feature = "full")))]
pub use elfo_dumper as dumper;
#[cfg(feature = "elfo-logger")]
#[cfg_attr(docsrs, doc(cfg(feature = "full")))]
pub use elfo_logger as logger;
#[cfg(feature = "test-util")]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub use elfo_test as test;

pub mod prelude {
    pub use super::{assert_msg, assert_msg_eq, message, msg, ActorGroup, Context, Schema};
}

#[deprecated(since = "0.1.1")]
pub mod actors {
    #[cfg(feature = "elfo-configurer")]
    #[cfg_attr(docsrs, doc(cfg(feature = "full")))]
    pub use super::configurer;
}
