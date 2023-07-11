#[macro_export]
macro_rules! assert_msg {
    ($envelope:expr, $pat:pat) => {{
        use $crate::_priv::{AnyMessageBorrowed, EnvelopeBorrowed};

        let envelope = &$envelope;
        let msg = envelope.unpack_regular().downcast2();

        #[allow(unreachable_patterns)]
        match &msg {
            &$pat => {}
            _ => panic!(
                "\na message doesn't match a pattern\npattern: {}\nmessage: {:#?}\n",
                stringify!($pat),
                msg,
            ),
        }
    }};
}

#[macro_export]
macro_rules! assert_msg_eq {
    ($envelope:expr, $expected:expr) => {{
        use $crate::_priv::{AnyMessageBorrowed, EnvelopeBorrowed};

        let envelope = &$envelope;

        let actual = envelope.unpack_regular().downcast2();
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
            match H::PROTOCOL {
                Some(protocol) => protocol,
                None => env!(
                    "CARGO_PKG_NAME",
                    "building without cargo is unsupported for now"
                ),
            }
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
