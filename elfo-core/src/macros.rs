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

macro_rules! cfg_network {
    // Force `{..}` to make rustfmt work.
    ({$($item:item)*}) => {
        $(
            #[cfg(feature = "network")]
            $item
        )*
    }
}
