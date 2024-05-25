use std::ops::Deref;

// See https://github.com/GoldsteinE/gh-blog/blob/master/const_deref_specialization/src/lib.md

// Reexported in `elfo::_priv`.
#[doc(hidden)]
pub struct ProtocolExtractor;

// Reexported in `elfo::_priv`.
#[doc(hidden)]
pub trait ProtocolHolder {
    const PROTOCOL: Option<&'static str>;
}

// Reexported in `elfo::_priv`.
#[doc(hidden)]
pub struct DefaultProtocolHolder;

impl ProtocolHolder for DefaultProtocolHolder {
    // `None` means a crate's name is used.
    const PROTOCOL: Option<&'static str> = None;
}

impl Deref for ProtocolExtractor {
    type Target = DefaultProtocolHolder;

    fn deref(&self) -> &Self::Target {
        &DefaultProtocolHolder
    }
}

impl DefaultProtocolHolder {
    pub fn holder(&self) -> Self {
        Self
    }
}

// TODO: add examples.
/// Overrides the protocol name for all messages in the current module.
/// Can be used only once per module. Submodules inherit this override
/// if `use super::*` is used.
#[macro_export]
macro_rules! set_protocol {
    ($protocol:literal) => {
        #[doc(hidden)]
        struct _ElfoProtocolHolder;
        impl $crate::_priv::ProtocolHolder for _ElfoProtocolHolder {
            const PROTOCOL: Option<&'static str> = Some($protocol);
        }

        #[doc(hidden)]
        trait _ElfoProtocolOverride {
            fn holder(&self) -> _ElfoProtocolHolder {
                _ElfoProtocolHolder
            }
        }
        impl _ElfoProtocolOverride for $crate::_priv::ProtocolExtractor {}
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! get_protocol {
    () => {{
        use $crate::_priv::{ProtocolExtractor, ProtocolHolder};

        const fn extract_protocol_name<H: ProtocolHolder>(_: &impl FnOnce() -> H) -> &'static str {
            // Get a protocol name from the relevant `set_protocol!` call.
            if let Some(proto_override) = H::PROTOCOL {
                return proto_override;
            }

            // If `set_protocol!` hasn't been used, take the package name.
            if let Some(pkg_name) = option_env!("CARGO_PKG_NAME") {
                return pkg_name;
            }

            panic!(
                "if the crate is built without cargo, \
                 the protocol must be set explicitly by calling `elfo::set_protocol!(..)`"
            );
        }

        extract_protocol_name(&|| ProtocolExtractor.holder())
    }};
}
