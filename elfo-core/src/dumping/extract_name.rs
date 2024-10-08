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
        // Skip private types (`$serde_json::private::{RawValue, Number}`)
        if name.starts_with("$serde_json") {
            return Err(Outcome::Inapplicable);
        }

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

/// Extract a name from the provided `Serialize` instance:
/// * for structs it returns the struct's name;
/// * for enums it returns `EnumName::VariantName`;
/// * fallback to [`extract_name_by_type`].
#[stability::unstable]
pub fn extract_name<S>(value: &S) -> MessageName
where
    S: Serialize + ?Sized,
{
    match value.serialize(NameExtractor).unwrap_err() {
        Outcome::Done(name) => name,
        Outcome::Inapplicable => extract_name_by_type::<S>(),
        Outcome::Error(_err) => extract_name_by_type::<S>(),
    }
}

/// Extract a name using [`std::any::type_name`] and some heuristics:
/// * known wrappers (`Option`, `Result`, `Box` and so on) are removed;
/// * for known collections returns an empty name;
/// * for primitives (integers and so on) returns an empty name;
/// * paths are stripped;
///
/// Prefer [`extract_name`] if possible.
#[stability::unstable]
pub fn extract_name_by_type<T: ?Sized>() -> MessageName {
    let s = std::any::type_name::<T>();

    let s = strip_known_wrappers(s);

    // Forbid collections.
    if should_be_forbidden(s) {
        return MessageName::default();
    }

    let s = extract_path(s);

    // Remove prefix (`prefix::LocalName`).
    let s = s.rsplit_once("::").map_or(s, |(_, s)| s);

    // Forbid primitives.
    if s.starts_with(char::is_lowercase) {
        return MessageName::default();
    }

    s.into()
}

fn strip_known_wrappers(mut s: &str) -> &str {
    while {
        let orig_len = s.len();

        s = s.strip_prefix('&').unwrap_or(s);
        s = s.strip_prefix("core::option::Option<").unwrap_or(s);
        s = s.strip_prefix("core::result::Result<").unwrap_or(s);
        s = s.strip_prefix("alloc::boxed::Box<").unwrap_or(s);
        s = s.strip_prefix("alloc::sync::Arc<").unwrap_or(s);
        s = s.strip_prefix("alloc::rc::Rc<").unwrap_or(s);
        s = s.strip_prefix("core::cell::Cell<").unwrap_or(s);
        s = s.strip_prefix("core::cell::RefCell<").unwrap_or(s);

        s.len() != orig_len
    } {}
    s
}

fn extract_path(s: &str) -> &str {
    if let Some(idx) = s.find(['<', '>', ',']) {
        &s[..idx]
    } else {
        s
    }
}

fn should_be_forbidden(s: &str) -> bool {
    s.starts_with("elfo_core::dumping::raw::Raw")
        || s.starts_with('[')
        || s.starts_with('(')
        || s.starts_with("std::collections::")
        || s.starts_with("alloc::vec::")

    // TODO: indexmap and so on?
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde::ser::Error as _;

    use super::*;

    fn extract_name_pretty(value: &impl Serialize) -> String {
        extract_name(value).to_string()
    }

    fn extract_name_by_type_pretty<T: ?Sized>() -> String {
        extract_name_by_type::<T>().to_string()
    }

    #[derive(Serialize)]
    struct Struct {
        n: u8,
    }

    #[derive(Serialize)]
    struct UnitStruct;

    #[derive(Serialize)]
    struct NewtypeStruct(u8);

    #[derive(Serialize)]
    struct TupleStruct(u8, u8);

    #[test]
    fn struct_() {
        assert_eq!(extract_name_pretty(&Struct { n: 42 }), "Struct");
        assert_eq!(extract_name_pretty(&UnitStruct), "UnitStruct");
        assert_eq!(extract_name_pretty(&NewtypeStruct(42)), "NewtypeStruct");
        assert_eq!(extract_name_pretty(&TupleStruct(42, 42)), "TupleStruct");
    }

    #[test]
    fn enum_() {
        #[derive(Serialize)]
        enum Enum {
            A(u8),
            A2(NewtypeStruct),
            B { n: u8 },
            B2(Struct),
            C(u8, u8),
            C2(TupleStruct),
            D,
            D2(UnitStruct),
        }

        assert_eq!(extract_name_pretty(&Enum::A(42)), "Enum::A");
        assert_eq!(
            extract_name_pretty(&Enum::A2(NewtypeStruct(42))),
            "Enum::A2"
        );
        assert_eq!(extract_name_pretty(&Enum::B { n: 42 }), "Enum::B");
        assert_eq!(extract_name_pretty(&Enum::B2(Struct { n: 42 })), "Enum::B2");
        assert_eq!(extract_name_pretty(&Enum::C(42, 42)), "Enum::C");
        assert_eq!(
            extract_name_pretty(&Enum::C2(TupleStruct(42, 42))),
            "Enum::C2"
        );
        assert_eq!(extract_name_pretty(&Enum::D), "Enum::D");
        assert_eq!(extract_name_pretty(&Enum::D2(UnitStruct)), "Enum::D2");
    }

    #[test]
    fn internally_tagged_enum() {
        #[derive(Serialize)]
        #[serde(tag = "kind")]
        enum Enum {
            A(u8),
            A2(NewtypeStruct),
            B { n: u8 },
            B2(Struct),
            C2(TupleStruct),
            D,
            D2(UnitStruct),
        }

        assert_eq!(extract_name_pretty(&Enum::A(42)), "Enum");
        assert_eq!(extract_name_pretty(&Enum::A2(NewtypeStruct(42))), "Enum");
        assert_eq!(extract_name_pretty(&Enum::B { n: 42 }), "Enum");
        assert_eq!(extract_name_pretty(&Enum::B2(Struct { n: 42 })), "Struct");
        assert_eq!(extract_name_pretty(&Enum::C2(TupleStruct(42, 42))), "Enum");
        assert_eq!(extract_name_pretty(&Enum::D), "Enum");
        assert_eq!(extract_name_pretty(&Enum::D2(UnitStruct)), "Enum");
    }

    #[test]
    fn externally_tagged_enum() {
        #[derive(Serialize)]
        #[serde(tag = "kind", content = "data")]
        enum Enum {
            A(u8),
            A2(NewtypeStruct),
            B { n: u8 },
            B2(Struct),
            C2(TupleStruct),
            D,
            D2(UnitStruct),
        }

        assert_eq!(extract_name_pretty(&Enum::A(42)), "Enum");
        assert_eq!(extract_name_pretty(&Enum::A2(NewtypeStruct(42))), "Enum");
        assert_eq!(extract_name_pretty(&Enum::B { n: 42 }), "Enum");
        assert_eq!(extract_name_pretty(&Enum::B2(Struct { n: 42 })), "Enum");
        assert_eq!(extract_name_pretty(&Enum::C2(TupleStruct(42, 42))), "Enum");
        assert_eq!(extract_name_pretty(&Enum::D), "Enum");
        assert_eq!(extract_name_pretty(&Enum::D2(UnitStruct)), "Enum");
    }

    #[test]
    fn untagged_enum() {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum Enum {
            A(u8),
            A2(NewtypeStruct),
            B { n: u8 },
            B2(Struct),
            C(u8, u8),
            C2(TupleStruct),
            D,
            D2(UnitStruct),
        }

        assert_eq!(extract_name_pretty(&Enum::A(42)), "Enum");
        assert_eq!(
            extract_name_pretty(&Enum::A2(NewtypeStruct(42))),
            "NewtypeStruct"
        );
        assert_eq!(extract_name_pretty(&Enum::B { n: 42 }), "Enum");
        assert_eq!(extract_name_pretty(&Enum::B2(Struct { n: 42 })), "Struct");
        assert_eq!(extract_name_pretty(&Enum::C(42, 42)), "Enum");
        assert_eq!(
            extract_name_pretty(&Enum::C2(TupleStruct(42, 42))),
            "TupleStruct"
        );
        assert_eq!(extract_name_pretty(&Enum::D), "Enum");
        assert_eq!(extract_name_pretty(&Enum::D2(UnitStruct)), "UnitStruct");
    }

    #[test]
    fn option() {
        #[derive(Serialize)]
        enum Enum {
            A,
        }

        assert_eq!(extract_name_pretty(&Some(Enum::A)), "Enum::A");

        // Should fallback to `extract_name_by_type`.
        assert_eq!(extract_name_pretty(&None::<Enum>), "Enum");
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

        use crate::dumping::Raw;
        assert_eq!(extract_name_pretty(&Raw("52".to_string())), "");
    }

    #[test]
    fn custom_error() {
        struct Foo;
        impl Serialize for Foo {
            fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(S::Error::custom("oops"))
            }
        }

        // Should fallback to `extract_name_by_type`.
        assert_eq!(extract_name_pretty(&Foo), "Foo");
    }

    #[test]
    fn by_type() {
        struct Simple;
        assert_eq!(extract_name_by_type_pretty::<Simple>(), "Simple");
        assert_eq!(extract_name_by_type_pretty::<&Simple>(), "Simple");
        assert_eq!(extract_name_by_type_pretty::<Option<Simple>>(), "Simple");
        assert_eq!(extract_name_by_type_pretty::<Option<&Simple>>(), "Simple");
        assert_eq!(extract_name_by_type_pretty::<&Option<&Simple>>(), "Simple");
        assert_eq!(
            extract_name_by_type_pretty::<Option<Option<Simple>>>(),
            "Simple"
        );
        assert_eq!(
            extract_name_by_type_pretty::<Option<Box<Simple>>>(),
            "Simple"
        );
        assert_eq!(
            extract_name_by_type_pretty::<&Option<&std::sync::Arc<&Simple>>>(),
            "Simple"
        );
        assert_eq!(
            extract_name_by_type_pretty::<Option<&std::rc::Rc<&std::cell::Cell<Simple>>>>(),
            "Simple"
        );
        assert_eq!(
            extract_name_by_type_pretty::<Option<&std::cell::RefCell<Simple>>>(),
            "Simple"
        );
        assert_eq!(
            extract_name_by_type_pretty::<&Result<&std::sync::Arc<&Simple>, Option<u32>>>(),
            "Simple"
        );

        // Parametrized.
        struct Param<P>(P);
        assert_eq!(extract_name_by_type_pretty::<Param<Simple>>(), "Param");
        assert_eq!(
            extract_name_by_type_pretty::<Option<Param<Simple>>>(),
            "Param"
        );
        assert_eq!(
            extract_name_by_type_pretty::<Option<Option<Param<Simple>>>>(),
            "Param"
        );

        // Primitives.
        assert_eq!(extract_name_by_type_pretty::<u8>(), "");
        assert_eq!(extract_name_by_type_pretty::<f64>(), "");

        // Collections.
        assert_eq!(extract_name_by_type_pretty::<&[u8]>(), "");
        assert_eq!(extract_name_by_type_pretty::<[u8]>(), "");
        assert_eq!(extract_name_by_type_pretty::<(Simple, Simple)>(), "");
        assert_eq!(
            extract_name_by_type_pretty::<Option<(Simple, Simple)>>(),
            ""
        );
        assert_eq!(extract_name_by_type_pretty::<HashMap<Simple, u32>>(), "");
        assert_eq!(extract_name_by_type_pretty::<Vec<Simple>>(), "");
    }
}
