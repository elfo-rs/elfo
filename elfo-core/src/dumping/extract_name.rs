use std::fmt;

use derive_more::{Display, Error};
use serde::{ser, Serialize, Serializer};

use super::dump::MessageName;

// === Outcome ===

#[derive(Debug, Display, Error)]
enum Outcome {
    Done(#[error(not(source))] MessageName),
    Inapplicable,
    Error(#[error(not(source))] String),
}

impl ser::Error for Outcome {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Error(msg.to_string())
    }
}

// === NameExtractor ===

type Result<T = (), E = Outcome> = std::result::Result<T, E>;
type Impossible = ser::Impossible<(), Outcome>;

struct NameExtractor;

macro_rules! inapplicable {
    ($ser_method:ident($ty:ty) -> $ret:ty, $($other:tt)*) => {
        #[inline]
        fn $ser_method(self, _v: $ty) -> $ret {
            Err(Outcome::Inapplicable)
        }
        inapplicable!($($other)*);
    };
    ($ser_method:ident($ty:ty), $($other:tt)*) => {
        inapplicable!($ser_method($ty) -> Result, $($other)*);
    };
    ($ser_method:ident, $($other:tt)*) => {
        #[inline]
        fn $ser_method(self) -> Result {
            Err(Outcome::Inapplicable)
        }
        inapplicable!($($other)*);
    };
    () => {};
}

impl Serializer for NameExtractor {
    type Error = Outcome;
    type Ok = ();
    type SerializeMap = Impossible;
    type SerializeSeq = Impossible;
    type SerializeStruct = Impossible;
    type SerializeStructVariant = Impossible;
    type SerializeTuple = Impossible;
    type SerializeTupleStruct = Impossible;
    type SerializeTupleVariant = Impossible;

    inapplicable!(
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_bool(bool),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
        serialize_none,
        serialize_unit,
        serialize_seq(Option<usize>) -> Result<Impossible>,
        serialize_tuple(usize) -> Result<Impossible>,
        serialize_map(Option<usize>) -> Result<Impossible>,
    );

    #[inline]
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result {
        value.serialize(self)
    }

    #[inline]
    fn serialize_unit_struct(self, name: &'static str) -> Result {
        Err(Outcome::Done(name.into()))
    }

    #[inline]
    fn serialize_unit_variant(
        self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result {
        Err(Outcome::Done((name, variant).into()))
    }

    #[inline]
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        _value: &T,
    ) -> Result {
        Err(Outcome::Done(name.into()))
    }

    #[inline]
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _value: &T,
    ) -> Result {
        Err(Outcome::Done((name, variant).into()))
    }

    #[inline]
    fn serialize_tuple_struct(self, name: &'static str, _len: usize) -> Result<Impossible> {
        Err(Outcome::Done(name.into()))
    }

    #[inline]
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Impossible> {
        Err(Outcome::Done((name, variant).into()))
    }

    #[inline]
    fn serialize_struct(self, name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        Err(Outcome::Done(name.into()))
    }

    #[inline]
    fn serialize_struct_variant(
        self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        Err(Outcome::Done((name, variant).into()))
    }

    #[inline]
    fn is_human_readable(&self) -> bool {
        true
    }
}

/// Extract a name from the provided `Serialize` instance.
///
/// Returns
/// * `Ok(true)` if the name is extracted successfully.
/// * `Ok(false)` if the name cannot be extracted.
/// * `Err(err)` if a custom error occurs.
#[stability::unstable]
pub fn extract_name(value: &impl Serialize) -> Result<MessageName, String> {
    match value.serialize(NameExtractor).unwrap_err() {
        Outcome::Done(name) => Ok(name),
        Outcome::Inapplicable => Ok("".into()),
        Outcome::Error(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::ser::Error as _;

    use super::*;

    fn extract_name_pretty(value: &impl Serialize) -> String {
        extract_name(value).unwrap().to_string()
    }

    #[test]
    fn struct_() {
        #[derive(Serialize)]
        struct Struct {
            n: u8,
        }
        assert_eq!(extract_name_pretty(&Struct { n: 42 }), "Struct");
    }

    #[test]
    fn unit_struct() {
        #[derive(Serialize)]
        struct UnitStruct;
        assert_eq!(extract_name_pretty(&UnitStruct), "UnitStruct");
    }

    #[test]
    fn newtype_struct() {
        #[derive(Serialize)]
        struct NewtypeStruct(u8);
        assert_eq!(extract_name_pretty(&NewtypeStruct(42)), "NewtypeStruct");
    }

    #[test]
    fn tuple_struct() {
        #[derive(Serialize)]
        struct TupleStruct(u8, u8);
        assert_eq!(extract_name_pretty(&TupleStruct(42, 42)), "TupleStruct");
    }

    #[test]
    fn enum_() {
        #[derive(Serialize)]
        enum Enum {
            A(u8),
            B { n: u8 },
            C(u8, u8),
            D,
        }

        assert_eq!(extract_name_pretty(&Enum::A(42)), "Enum::A");
        assert_eq!(extract_name_pretty(&Enum::B { n: 42 }), "Enum::B");
        assert_eq!(extract_name_pretty(&Enum::C(42, 42)), "Enum::C");
        assert_eq!(extract_name_pretty(&Enum::D), "Enum::D");
    }

    #[test]
    fn some() {
        #[derive(Serialize)]
        struct Struct;
        assert_eq!(extract_name_pretty(&Some(Struct)), "Struct");
    }

    #[test]
    fn inapplicable() {
        assert_eq!(extract_name_pretty(&42u8), "");
        assert_eq!(extract_name_pretty(&42f64), "");
        assert_eq!(extract_name_pretty(&"foobar"), "");
        assert_eq!(extract_name_pretty(&[42]), "");
        assert_eq!(extract_name_pretty(&None::<u32>), "");
        assert_eq!(extract_name_pretty(&HashMap::<u32, u32>::default()), "");
        assert_eq!(extract_name_pretty(&()), "");
        assert_eq!(extract_name_pretty(&(42, 42)), "");
    }

    #[test]
    fn custom_error() {
        struct Foo;
        impl Serialize for Foo {
            fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(S::Error::custom("oops"))
            }
        }

        assert_eq!(extract_name(&Foo).unwrap_err(), "oops");
    }
}
