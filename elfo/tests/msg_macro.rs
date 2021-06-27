use elfo::{config::AnyConfig, prelude::*};

#[message]
struct Unit;
#[message(ret = Type)]
struct ReqUnit;

#[message]
struct Tuple(u32, u32);
#[message(ret = Type)]
struct ReqTuple(u32, u32);

#[message]
struct Struct {
    a: u32,
}
#[message(ret = Type)]
struct ReqStruct {
    a: u32,
}

#[message]
enum Enum {
    A { a: u32 },
    B,
}
#[message(ret = Type)]
enum ReqEnum {
    A { a: u32 },
    B,
}

#[message]
#[derive(PartialEq)]
enum Type {
    Unit(u32),
    Tuple(u32),
    Struct(u32),
    Enum(u32),
}

fn sample() -> Schema {
    ActorGroup::new().exec(|mut ctx| async move {
        while let Some(envelope) = ctx.recv().await {
            msg!(match &envelope {
                Unit | Tuple | Struct | ReqStruct => true,
                _ => false,
            });

            msg!(match envelope {
                // Unit.
                Unit => ctx.send(Type::Unit(0)).await.unwrap(),
                (ReqUnit, token) => ctx.respond(token, Type::Unit(1)),

                // Tuple.
                Tuple(a, _) if a == 0 => ctx.send(Type::Tuple(0)).await.unwrap(),
                _t @ Tuple => ctx.send(Type::Tuple(1)).await.unwrap(),
                (ReqTuple(a, _), token) if a == 0 => ctx.respond(token, Type::Tuple(2)),
                (_t @ ReqTuple, token) => ctx.respond(token, Type::Tuple(3)),

                // Struct.
                Struct { a } if a == 0 => ctx.send(Type::Struct(0)).await.unwrap(),
                Struct => ctx.send(Type::Struct(1)).await.unwrap(),
                (ReqStruct { a }, token) if a == 0 => ctx.respond(token, Type::Struct(2)),
                (_t @ ReqStruct, token) => ctx.respond(token, Type::Struct(3)),

                // Enum.
                Enum::A { a } if a == 0 => ctx.send(Type::Enum(0)).await.unwrap(),
                Enum::B => ctx.send(Type::Enum(1)).await.unwrap(),
                (ReqEnum::A { a: _a }, token) => ctx.respond(token, Type::Enum(2)),
                (_t @ ReqEnum::B, token) => ctx.respond(token, Type::Enum(3)),
                e @ Enum => ctx
                    .send(match e {
                        Enum::A { .. } => Type::Enum(4),
                        Enum::B => unreachable!(),
                    })
                    .await
                    .unwrap(),
            });
        }
    })
}

#[tokio::test]
async fn it_handles_unit() {
    let mut proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    proxy.send(Unit).await;
    assert_msg_eq!(proxy.recv().await, Type::Unit(0));
    assert_eq!(proxy.request(ReqUnit).await, Type::Unit(1));
}

#[tokio::test]
async fn it_handles_tuple() {
    let mut proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    proxy.send(Tuple(0, 42)).await;
    assert_msg_eq!(proxy.recv().await, Type::Tuple(0));
    proxy.send(Tuple(1, 42)).await;
    assert_msg_eq!(proxy.recv().await, Type::Tuple(1));
    assert_eq!(proxy.request(ReqTuple(0, 42)).await, Type::Tuple(2));
    assert_eq!(proxy.request(ReqTuple(1, 42)).await, Type::Tuple(3));
}

#[tokio::test]
async fn it_handles_struct() {
    let mut proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    proxy.send(Struct { a: 0 }).await;
    assert_msg_eq!(proxy.recv().await, Type::Struct(0));
    proxy.send(Struct { a: 1 }).await;
    assert_msg_eq!(proxy.recv().await, Type::Struct(1));
    assert_eq!(proxy.request(ReqStruct { a: 0 }).await, Type::Struct(2));
    assert_eq!(proxy.request(ReqStruct { a: 1 }).await, Type::Struct(3));
}

#[tokio::test]
async fn it_handles_enum() {
    let mut proxy = elfo::test::proxy(sample(), AnyConfig::default()).await;
    proxy.send(Enum::A { a: 0 }).await;
    assert_msg_eq!(proxy.recv().await, Type::Enum(0));
    proxy.send(Enum::B).await;
    assert_msg_eq!(proxy.recv().await, Type::Enum(1));
    assert_eq!(proxy.request(ReqEnum::A { a: 0 }).await, Type::Enum(2));
    assert_eq!(proxy.request(ReqEnum::B).await, Type::Enum(3));
    proxy.send(Enum::A { a: 1 }).await;
    assert_msg_eq!(proxy.recv().await, Type::Enum(4));
}
