use std::fmt;

use derive_more::{Display, Error};
use serde::{ser, Serialize, Serializer};

// === Outcome ===

#[derive(Debug, Display, Error)]
enum Outcome {
    Done,
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

struct NameExtractor<'a>(&'a mut String);

impl<'a> NameExtractor<'a> {
    fn struct_name(&mut self, name: &str) {
        self.0.push_str(name);
    }

    fn variant_name(&mut self, enum_name: &str, variant_name: &str) {
        self.0.push_str(enum_name);
        self.0.push_str("::");
        self.0.push_str(variant_name);
    }
}

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

impl<'a> Serializer for NameExtractor<'a> {
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
    fn serialize_unit_struct(mut self, name: &'static str) -> Result {
        self.struct_name(name);
        Err(Outcome::Done)
    }

    #[inline]
    fn serialize_unit_variant(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result {
        self.variant_name(name, variant);
        Err(Outcome::Done)
    }

    #[inline]
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        mut self,
        name: &'static str,
        _value: &T,
    ) -> Result {
        self.struct_name(name);
        Err(Outcome::Done)
    }

    #[inline]
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _value: &T,
    ) -> Result {
        self.variant_name(name, variant);
        Err(Outcome::Done)
    }

    #[inline]
    fn serialize_tuple_struct(mut self, name: &'static str, _len: usize) -> Result<Impossible> {
        self.struct_name(name);
        Err(Outcome::Done)
    }

    #[inline]
    fn serialize_tuple_variant(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Impossible> {
        self.variant_name(name, variant);
        Err(Outcome::Done)
    }

    #[inline]
    fn serialize_struct(
        mut self,
        name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct> {
        self.struct_name(name);
        Err(Outcome::Done)
    }

    #[inline]
    fn serialize_struct_variant(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        self.variant_name(name, variant);
        Err(Outcome::Done)
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
pub(crate) fn extract_name_into(s: &mut String, value: &impl Serialize) -> Result<bool, String> {
    match value.serialize(NameExtractor(s)).unwrap_err() {
        Outcome::Done => Ok(true),
        Outcome::Inapplicable => Ok(false),
        Outcome::Error(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::ser::Error as _;

    use super::*;

    #[test]
    fn struct_() {
        #[derive(Serialize)]
        struct Struct {
            n: u8,
        }
        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &Struct { n: 42 }), Ok(true));
        assert_eq!(s, "Struct");
    }

    #[test]
    fn unit_struct() {
        #[derive(Serialize)]
        struct UnitStruct;
        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &UnitStruct), Ok(true));
        assert_eq!(s, "UnitStruct");
    }

    #[test]
    fn newtype_struct() {
        #[derive(Serialize)]
        struct NewtypeStruct(u8);
        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &NewtypeStruct(42)), Ok(true));
        assert_eq!(s, "NewtypeStruct");
    }

    #[test]
    fn tuple_struct() {
        #[derive(Serialize)]
        struct TupleStruct(u8, u8);
        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &TupleStruct(42, 42)), Ok(true));
        assert_eq!(s, "TupleStruct");
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

        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &Enum::A(42)), Ok(true));
        assert_eq!(s, "Enum::A");

        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &Enum::B { n: 42 }), Ok(true));
        assert_eq!(s, "Enum::B");

        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &Enum::C(42, 42)), Ok(true));
        assert_eq!(s, "Enum::C");

        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &Enum::D), Ok(true));
        assert_eq!(s, "Enum::D");
    }

    #[test]
    fn some() {
        #[derive(Serialize)]
        struct Struct;
        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &Some(Struct)), Ok(true));
        assert_eq!(s, "Struct");
    }

    #[test]
    fn inapplicable() {
        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &42u8), Ok(false));
        assert_eq!(extract_name_into(&mut s, &42f64), Ok(false));
        assert_eq!(extract_name_into(&mut s, &"foobar"), Ok(false));
        assert_eq!(extract_name_into(&mut s, &[42]), Ok(false));
        assert_eq!(extract_name_into(&mut s, &None::<u32>), Ok(false));
        assert_eq!(
            extract_name_into(&mut s, &HashMap::<u32, u32>::default()),
            Ok(false)
        );
        assert_eq!(extract_name_into(&mut s, &()), Ok(false));
        assert_eq!(extract_name_into(&mut s, &(42, 42)), Ok(false));
        assert!(s.is_empty());
    }

    #[test]
    fn custom_error() {
        struct Foo;
        impl Serialize for Foo {
            fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(S::Error::custom("oops"))
            }
        }

        let mut s = String::new();
        assert_eq!(extract_name_into(&mut s, &Foo), Err("oops".to_string()));
        assert!(s.is_empty());
    }
}
