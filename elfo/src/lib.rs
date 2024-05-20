//! An actor system for Rust with batteries included.
//!
//! See [The Actoromicon](https://actoromicon.rs/) for more information.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub use elfo_core::*;
pub use elfo_macros::{message, msg};

#[cfg(feature = "test-util")]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub use elfo_test as test;

/// A set of actors for common tasks.
pub mod batteries {
    #[cfg(feature = "elfo-configurer")]
    #[cfg_attr(docsrs, doc(cfg(feature = "full")))]
    pub use elfo_configurer as configurer;
    #[cfg(feature = "elfo-dumper")]
    #[cfg_attr(docsrs, doc(cfg(feature = "full")))]
    pub use elfo_dumper as dumper;
    #[cfg(feature = "elfo-logger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "full")))]
    pub use elfo_logger as logger;
    #[cfg(feature = "elfo-network")] // TODO
    #[cfg_attr(docsrs, doc(cfg(feature = "full")))]
    pub use elfo_network as network;
    #[cfg(feature = "elfo-pinger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "full")))]
    pub use elfo_pinger as pinger;
    #[cfg(feature = "elfo-telemeter")]
    #[cfg_attr(docsrs, doc(cfg(feature = "full")))]
    pub use elfo_telemeter as telemeter;
}

/// Things that are useful to have included when writing actors.
pub mod prelude {
    pub use super::{
        assert_msg, assert_msg_eq, message, msg, ActorGroup, Blueprint, Context, SourceHandle,
    };
}
