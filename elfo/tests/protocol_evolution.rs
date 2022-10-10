#![cfg(feature = "network")]

use std::collections::HashMap;

use anyhow::{anyhow, ensure, Result};

use elfo::{_priv::AnyMessage, prelude::*, Message};

fn parse<A: Message + PartialEq, B: Message + PartialEq>(input: A, expected: B) -> Result<()> {
    let mut buf = [0; 128];
    let input = AnyMessage::new(input);
    let size = input.write_msgpack(&mut buf)?;
    let actual = AnyMessage::read_msgpack(B::VTABLE.protocol, B::VTABLE.name, &buf[..size])?
        .ok_or_else(|| anyhow!("no such message"))?
        .downcast::<B>()
        .map_err(|_| anyhow!("cannot downcast"))?;

    ensure!(actual == expected);
    Ok(())
}

#[track_caller]
fn ensure_parsable<A: Message + PartialEq, B: Message + PartialEq>(input: A, expected: B) {
    parse(input, expected).unwrap();
}

#[track_caller]
fn ensure_unparsable<A: Message + PartialEq, B: Message + PartialEq>(input: A, expected: B) {
    parse(input, expected).unwrap_err();
}

#[message]
#[derive(PartialEq, Eq)]
struct S0 {
    a: u32,
}

#[test]
fn struct_new_required_field() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct S1 {
        a: u32,
        b: u32,
    }

    ensure_parsable(S1 { a: 42, b: 42 }, S0 { a: 42 });
    ensure_unparsable(S0 { a: 42 }, S1 { a: 42, b: 42 });
}

#[test]
fn struct_new_optional_field() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct S2 {
        a: u32,
        b: Option<u32>,
    }

    ensure_parsable(S2 { a: 42, b: Some(42) }, S0 { a: 42 });
    ensure_parsable(S0 { a: 42 }, S2 { a: 42, b: None });
}

#[test]
fn struct_new_default_field() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct S3 {
        a: u32,
        #[serde(default)]
        b: u32,
    }

    ensure_parsable(S3 { a: 42, b: 42 }, S0 { a: 42 });
    ensure_parsable(S0 { a: 42 }, S3 { a: 42, b: 0 });
}

#[test]
fn struct_aliased_field() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct S4 {
        #[serde(alias = "a")]
        b: u32,
    }

    ensure_parsable(S0 { a: 42 }, S4 { b: 42 });
    ensure_unparsable(S4 { b: 42 }, S0 { a: 42 });
}

#[test]
fn struct_renamed_field() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct S5 {
        #[serde(rename = "a")]
        #[serde(alias = "b")]
        b: u32,
    }

    #[message]
    #[derive(PartialEq, Eq)]
    struct S6 {
        #[serde(alias = "a")]
        b: u32,
    }

    ensure_parsable(S0 { a: 42 }, S5 { b: 42 });
    ensure_parsable(S5 { b: 42 }, S0 { a: 42 });
    ensure_parsable(S5 { b: 42 }, S6 { b: 42 });
}

#[test]
fn num_promotion() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct S7 {
        a: u64,
    }

    #[message]
    #[derive(PartialEq, Eq)]
    struct S8 {
        a: u8,
    }

    ensure_parsable(S0 { a: 42 }, S7 { a: 42 });
    ensure_parsable(S7 { a: 42 }, S0 { a: 42 });
    ensure_parsable(S0 { a: 42 }, S8 { a: 42 });
    ensure_parsable(S8 { a: 42 }, S0 { a: 42 });
}

#[test]
fn maps_with_arbitrary_keys() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct SM0 {
        map: HashMap<Key0, u32>,
    }

    #[message(part)]
    #[derive(PartialEq, Eq, Hash)]
    struct Key0 {
        a: String,
    }

    #[message]
    #[derive(PartialEq, Eq)]
    struct SM1 {
        map: HashMap<Key1, u32>,
    }

    #[message(part)]
    #[derive(PartialEq, Eq, Hash)]
    struct Key1 {
        a: String,
        #[serde(default)]
        b: u32,
    }

    let map0 = vec![(Key0 { a: "a".into() }, 42)]
        .into_iter()
        .collect::<HashMap<_, _>>();
    let map1 = vec![(
        Key1 {
            a: "a".into(),
            b: 0,
        },
        42,
    )]
    .into_iter()
    .collect::<HashMap<_, _>>();

    ensure_parsable(SM0 { map: map0.clone() }, SM1 { map: map1.clone() });
    ensure_parsable(SM1 { map: map1 }, SM0 { map: map0 });
}

#[test]
fn special_floats() {
    #[message]
    struct Float(f64);

    impl PartialEq for Float {
        fn eq(&self, other: &Self) -> bool {
            self.0.is_nan() && other.0.is_nan() || self.0 == other.0
        }
    }

    ensure_parsable(Float(f64::INFINITY), Float(f64::INFINITY));
    ensure_parsable(Float(f64::NAN), Float(f64::NAN));
}

#[test]
fn i128() {
    #[message]
    #[derive(PartialEq)]
    struct S128 {
        a: i128,
    }

    ensure_parsable(S128 { a: 42 }, S128 { a: 42 });
}

#[test]
fn newtype() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct S9 {
        a: U32,
    }

    #[message(part)]
    #[derive(PartialEq, Eq)]
    struct U32(u32);

    ensure_parsable(S0 { a: 42 }, S9 { a: U32(42) });
    ensure_parsable(S9 { a: U32(42) }, S0 { a: 42 });
}

// TODO: binary

#[message]
#[derive(PartialEq, Eq)]
enum E0 {
    A(u32),
}

#[test]
fn enum_new_variant() {
    #[message]
    #[derive(PartialEq, Eq)]
    enum E1 {
        A(u32),
        B(u32),
    }

    ensure_parsable(E1::A(42), E0::A(42));
    ensure_parsable(E0::A(42), E1::A(42));
    ensure_unparsable(E1::B(42), E0::A(42));
}

#[test]
fn enum_new_other_variant() {
    #[message]
    #[derive(PartialEq, Eq)]
    enum E2 {
        A,
        B,
    }

    #[message]
    #[derive(PartialEq, Eq)]
    enum E3 {
        A,
        #[serde(other)]
        O,
    }

    ensure_parsable(E2::B, E3::O);
}

#[test]
fn enum_aliased_variant() {
    #[message]
    #[derive(PartialEq, Eq)]
    enum E4 {
        #[serde(alias = "A")]
        B(u32),
    }

    ensure_parsable(E0::A(42), E4::B(42));
    ensure_unparsable(E4::B(42), E0::A(42));
}

#[test]
fn enum_renamed_variant() {
    #[message]
    #[derive(PartialEq, Eq)]
    enum E5 {
        #[serde(rename = "A")]
        #[serde(alias = "B")]
        B(u32),
    }

    #[message]
    #[derive(PartialEq, Eq)]
    enum E6 {
        #[serde(alias = "A")]
        B(u32),
    }

    ensure_parsable(E0::A(42), E5::B(42));
    ensure_parsable(E5::B(42), E0::A(42));
    ensure_parsable(E5::B(42), E6::B(42));
}

#[test]
fn enum_untagged() {
    #[message]
    #[derive(PartialEq, Eq)]
    struct SE0 {
        a: u32,
    }

    #[message]
    #[derive(PartialEq, Eq)]
    struct SE1 {
        a: A,
    }

    #[message(part)]
    #[derive(PartialEq, Eq)]
    #[serde(untagged)]
    enum A {
        Num(u32),
        Str(String),
    }

    ensure_parsable(SE0 { a: 42 }, SE1 { a: A::Num(42) });
    ensure_parsable(SE1 { a: A::Num(42) }, SE0 { a: 42 });
}
