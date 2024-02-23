use elfo_core::{dumping::extract_name_by_type, msg, Envelope, Message, Request, ResponseToken};

#[track_caller]
pub fn expect_message<T: Message>(msg: Envelope) -> T {
    msg!(match msg {
        msg @ T => msg,
        msg => panic!(
            r#"unexpected message: expected "{}", got "{}""#,
            extract_name_by_type::<T>(),
            msg.message().name()
        ),
    })
}

#[track_caller]
pub fn expect_request<T: Request>(msg: Envelope) -> (T, ResponseToken<T>) {
    msg!(match msg {
        (msg @ T, token) => (msg, token),
        msg => panic!(
            r#"unexpected request: expected "{}", got "{}""#,
            extract_name_by_type::<T>(),
            msg.message().name()
        ),
    })
}
