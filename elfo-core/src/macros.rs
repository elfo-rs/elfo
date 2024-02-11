#[macro_export]
macro_rules! assert_msg {
    ($envelope:expr, $pat:pat) => {{
        let envelope = &$envelope;
        let message = envelope.message();

        // TODO: use `msg!` to support multiple messages in a pattern.
        #[allow(unreachable_patterns)]
        match &message.downcast_ref() {
            Some($pat) => {}
            _ => panic!(
                "\na message doesn't match a pattern\npattern: {}\nmessage: {message:#?}\n",
                stringify!($pat),
            ),
        }
    }};
}

#[macro_export]
macro_rules! assert_msg_eq {
    ($envelope:expr, $expected:expr) => {{
        let envelope = &$envelope;

        let Some(actual) = envelope.message().downcast_ref() else {
            panic!("unexpected message: {:#?}", envelope.message());
        };
        let expected = &$expected;

        fn unify<T>(_rhs: &T, _lhs: &T) {}
        unify(actual, expected);

        assert_eq!(actual, expected);
    }};
}

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

// See https://github.com/GoldsteinE/gh-blog/blob/master/const_deref_specialization/src/lib.md
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

macro_rules! cfg_network {
    // Force `{..}` to make rustfmt work.
    ({$($item:item)*}) => {
        $(
            #[cfg(feature = "network")]
            $item
        )*
    }
}
