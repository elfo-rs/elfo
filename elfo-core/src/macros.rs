#[macro_export]
macro_rules! assert_msg {
    ($envelope:expr, $pat:pat) => {{
        use $crate::_priv::{AnyMessageBorrowed, EnvelopeBorrowed};

        let envelope = &$envelope;
        let msg = envelope.unpack_regular().downcast2();
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
