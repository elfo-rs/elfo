use std::{
    any::{Any, TypeId},
    fmt, mem,
    ops::Deref,
    sync::Arc,
};

use derive_more::From;
use serde::{de, de::value::Error as DeError, Deserialize, Deserializer, Serialize, Serializer};
use serde_value::{Value, ValueDeserializer};

use crate::local::Local;

pub trait Config: for<'de> Deserialize<'de> + Send + Sync + fmt::Debug + 'static {}
impl<C> Config for C where C: for<'de> Deserialize<'de> + Send + Sync + fmt::Debug + 'static {}

assert_impl_all!((): Config);

#[derive(Clone)]
pub struct AnyConfig {
    raw: Arc<Value>,
    // Actually, we store `Arc<Arc<C>>` here.
    decoded: Option<Local<Arc<dyn Any + Send + Sync>>>,
}

impl AnyConfig {
    pub(crate) fn new(value: Value) -> Self {
        Self {
            raw: Arc::new(value),
            decoded: None,
        }
    }

    pub(crate) fn get<C: 'static>(&self) -> Option<&Arc<C>> {
        self.decoded.as_ref().and_then(|local| local.downcast_ref())
    }

    pub(crate) fn decode<C: Config>(&self) -> Result<AnyConfig, String> {
        // Handle the special case of default config.
        let decoded = if TypeId::of::<C>() == TypeId::of::<()>() {
            Arc::new(Arc::new(())) as Arc<_>
        } else {
            let de = ValueDeserializer::<DeError>::new((*self.raw).clone());
            let config = C::deserialize(de).map_err(|err| err.to_string())?;
            Arc::new(Arc::new(config)) as Arc<_>
        };

        Ok(AnyConfig {
            raw: self.raw.clone(),
            decoded: Some(Local::from(decoded)),
        })
    }

    pub(crate) fn into_value(mut self) -> Value {
        mem::replace(Arc::make_mut(&mut self.raw), Value::Unit)
    }
}

impl Default for AnyConfig {
    fn default() -> Self {
        Self::new(Value::Map(Default::default()))
    }
}

impl fmt::Debug for AnyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Configs can contain credentials, so we should never print unknown configs.
        f.write_str("..")
    }
}

impl<'de> Deserialize<'de> for AnyConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::new)
    }
}

impl Serialize for AnyConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserializer<'de> for AnyConfig {
    type Error = serde_value::DeserializerError;

    serde::forward_to_deserialize_any! {
        bool u8 u16 u32 u64 i8 i16 i32 i64 f32 f64 char str string unit
        seq bytes byte_buf map unit_struct
        tuple_struct struct tuple ignored_any identifier
    }

    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.into_value().deserialize_any(visitor)
    }

    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.into_value().deserialize_option(visitor)
    }

    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.into_value().deserialize_enum(name, variants, visitor)
    }

    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.into_value().deserialize_newtype_struct(name, visitor)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, From)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Secret<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<secret>")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<secret>")
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self)
    }
}

impl<T: Serialize> Serialize for Secret<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // TODO: it should depend on the context (network or dumping).
        serializer.serialize_str("<secret>")
    }
}
