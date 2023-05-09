#![cfg(feature = "test-util")]

use std::{collections::HashMap, sync::Arc, time::Duration};

use derive_more::Constructor;
use elfo::{config::AnyConfig, prelude::*, scope, stream::Stream, tracing::TraceId};
use futures::StreamExt;
use parking_lot::Mutex;
use tokio::time;

#[tokio::test(start_paused = true)]
async fn once() {
    #[message]
    #[derive(Constructor)]
    struct Start {
        no: u64,
        override_trace_id: bool,
    }

    #[message]
    #[derive(PartialEq, Eq)]
    struct Finished(u64);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let mut trace_ids = HashMap::new();
        let mut terminated = None::<Stream<_>>;

        while let Some(envelope) = ctx.recv().await {
            if let Some(handle) = terminated.take() {
                assert!(handle.is_terminated());
            }

            msg!(match envelope {
                msg @ Start => {
                    let override_trace_id = msg.override_trace_id.then(TraceId::generate);
                    let no = msg.no;

                    let handle = ctx.attach(Stream::once(async move {
                        if let Some(trace_id) = override_trace_id {
                            scope::set_trace_id(trace_id);
                        }

                        time::sleep(Duration::from_millis(no)).await;
                        Finished(no)
                    }));

                    assert!(!handle.is_terminated());
                    let data = (handle, override_trace_id.unwrap_or_else(scope::trace_id));
                    trace_ids.insert(no, data);
                }
                msg @ Finished(no) => {
                    let (handle, expected_trace_id) = trace_ids.remove(&no).unwrap();
                    assert_eq!(scope::trace_id(), expected_trace_id);
                    ctx.send(msg).await.unwrap();

                    assert!(!handle.is_terminated()); // become after the next `recv()`
                    assert!(terminated.is_none());
                    terminated = Some(handle);
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert!(proxy.try_recv().is_none());

    proxy.send(Start::new(100, false)).await;
    proxy.send(Start::new(25, true)).await;
    proxy.send(Start::new(50, false)).await;

    assert_msg_eq!(proxy.recv().await, Finished(25));
    proxy.send(Start::new(5, false)).await;

    assert_msg_eq!(proxy.recv().await, Finished(5));
    assert_msg_eq!(proxy.recv().await, Finished(50));
    assert_msg_eq!(proxy.recv().await, Finished(100));
    assert!(proxy.try_recv().is_none());

    proxy.send(Start::new(10, true)).await;
    proxy.send(Start::new(15, false)).await;
    assert_msg_eq!(proxy.recv().await, Finished(10));
    assert_msg_eq!(proxy.recv().await, Finished(15));
}

#[tokio::test(start_paused = true)]
async fn from_futures03() {
    #[message]
    #[derive(Constructor)]
    struct Start {
        group: u64,
        count: u32,
    }

    #[message]
    #[derive(PartialEq, Eq, Constructor)]
    struct Produced {
        group: u64,
        no: u32,
    }

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let trace_ids = Arc::new(Mutex::new(HashMap::new()));

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                msg @ Start => {
                    let trace_ids = trace_ids.clone();

                    let stream = futures::stream::iter(0..msg.count).then(move |no| {
                        let trace_ids = trace_ids.clone();
                        let group = msg.group;

                        async move {
                            let expected_trace_id = if no % 2 == 0 {
                                let trace_id = TraceId::generate();
                                scope::set_trace_id(trace_id);
                                trace_id
                            } else {
                                scope::trace_id()
                            };

                            trace_ids.lock().insert((group, no), expected_trace_id);

                            time::sleep(Duration::from_millis(group)).await;
                            Produced::new(group, no)
                        }
                    });

                    ctx.attach(Stream::from_futures03(stream));
                }
                msg @ Produced => {
                    let expected_trace_id = trace_ids.lock().remove(&(msg.group, msg.no)).unwrap();
                    assert_eq!(scope::trace_id(), expected_trace_id);
                    ctx.send(msg).await.unwrap();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert!(proxy.try_recv().is_none());

    proxy.send(Start::new(11, 4)).await;
    proxy.send(Start::new(35, 3)).await;
    proxy.send(Start::new(49, 2)).await;

    assert_msg_eq!(proxy.recv().await, Produced::new(11, 0)); // 11
    assert_msg_eq!(proxy.recv().await, Produced::new(11, 1)); // 22
    assert_msg_eq!(proxy.recv().await, Produced::new(11, 2)); // 33
    assert_msg_eq!(proxy.recv().await, Produced::new(35, 0)); // 35
    assert_msg_eq!(proxy.recv().await, Produced::new(11, 3)); // 44
    assert_msg_eq!(proxy.recv().await, Produced::new(49, 0)); // 49
    assert_msg_eq!(proxy.recv().await, Produced::new(35, 1)); // 70
    assert_msg_eq!(proxy.recv().await, Produced::new(49, 1)); // 98
    assert_msg_eq!(proxy.recv().await, Produced::new(35, 2)); // 105
}

#[tokio::test(start_paused = true)]
async fn terminate() {
    #[message]
    struct Start(u64);

    #[message]
    struct Terminate(u64);

    #[message]
    #[derive(PartialEq, Eq)]
    struct Produced(u64);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let mut handles = HashMap::new();

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                Start(group) => {
                    let stream = futures::stream::unfold(0, move |no| async move {
                        time::sleep(Duration::from_millis(group)).await;
                        Some((Produced(group), no + 1))
                    });

                    let stream = ctx.attach(Stream::from_futures03(stream));
                    handles.insert(group, stream);
                }
                Terminate(group) => {
                    let handle = handles.remove(&group).unwrap();
                    assert!(!handle.is_terminated());
                    handle.terminate();
                }
                Produced(group) => {
                    ctx.send(Produced(group)).await.unwrap();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert!(proxy.try_recv().is_none());

    proxy.send(Start(11)).await;
    proxy.send(Start(23)).await;
    proxy.send(Start(35)).await;

    assert_msg_eq!(proxy.recv().await, Produced(11)); // 11
    assert_msg_eq!(proxy.recv().await, Produced(11)); // 22
    assert_msg_eq!(proxy.recv().await, Produced(23)); // 23
    assert_msg_eq!(proxy.recv().await, Produced(11)); // 33
    proxy.send(Terminate(11)).await;
    assert_msg_eq!(proxy.recv().await, Produced(35)); // 35
    assert_msg_eq!(proxy.recv().await, Produced(23)); // 46
    proxy.send(Terminate(23)).await;
    assert_msg_eq!(proxy.recv().await, Produced(35)); // 70
    proxy.send(Terminate(35)).await;

    assert!(proxy.try_recv().is_none());
}

#[tokio::test(start_paused = true)]
async fn generate() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct OneProduced(u32);

    #[message]
    #[derive(PartialEq, Eq)]
    struct TwoProduced(u32);

    #[message]
    #[derive(PartialEq, Eq)]
    struct Finished(u32);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        let trace_ids = Arc::new(Mutex::new(HashMap::new()));
        let trace_ids_1 = trace_ids.clone();

        ctx.attach(Stream::generate(|mut emitter| async move {
            trace_ids.lock().insert(0, scope::trace_id());
            trace_ids.lock().insert(1, scope::trace_id());
            emitter.emit(OneProduced(0)).await;
            emitter.emit(OneProduced(1)).await;

            scope::set_trace_id(TraceId::generate());
            trace_ids.lock().insert(2, scope::trace_id());
            trace_ids.lock().insert(3, scope::trace_id());
            emitter.emit(TwoProduced(2)).await;
            emitter.emit(TwoProduced(3)).await;

            scope::set_trace_id(TraceId::generate());
            trace_ids.lock().insert(4, scope::trace_id());
            emitter.emit(Finished(4)).await;
        }));

        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                msg @ OneProduced(no) | msg @ TwoProduced(no) | msg @ Finished(no) => {
                    let expected_trace_id = trace_ids_1.lock().remove(&no).unwrap();
                    assert_eq!(scope::trace_id(), expected_trace_id);
                    ctx.send(msg).await.unwrap();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;

    assert_msg_eq!(proxy.recv().await, OneProduced(0));
    assert_msg_eq!(proxy.recv().await, OneProduced(1));
    assert_msg_eq!(proxy.recv().await, TwoProduced(2));
    assert_msg_eq!(proxy.recv().await, TwoProduced(3));
    assert_msg_eq!(proxy.recv().await, Finished(4));
}

#[tokio::test(start_paused = true)]
async fn result() {
    #[message]
    #[derive(Constructor)]
    struct Start {
        no: u64,
        fail: bool,
    }

    #[message]
    #[derive(PartialEq, Eq)]
    struct Success(u64);

    #[message]
    #[derive(PartialEq, Eq)]
    struct Failure(u64);

    let group = ActorGroup::new().exec(|mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match envelope {
                msg @ Start => {
                    ctx.attach(Stream::once(async move {
                        time::sleep(Duration::from_millis(msg.no)).await;

                        if msg.fail {
                            Ok(Failure(msg.no))
                        } else {
                            Err(Success(msg.no))
                        }
                    }));
                }
                msg @ Success | msg @ Failure => {
                    ctx.send(msg).await.unwrap();
                }
            });
        }
    });

    let mut proxy = elfo::test::proxy(group, AnyConfig::default()).await;
    assert!(proxy.try_recv().is_none());

    proxy.send(Start::new(10, false)).await;
    proxy.send(Start::new(20, true)).await;

    assert_msg_eq!(proxy.recv().await, Success(10));
    assert_msg_eq!(proxy.recv().await, Failure(20));
}
