use std::{
    any::{Any, TypeId},
    fmt,
    sync::Arc,
};

use serde::{de::value::Error as DeError, Deserialize, Serialize};
use serde_value::{Value, ValueDeserializer};

use crate::local::Local;

#[derive(Clone, Serialize, Deserialize)]
pub struct AnyConfig {
    raw: Arc<Value>,
    // Actually, we store `Arc<Arc<C>>` here.
    decoded: Option<Local<Arc<dyn Any + Send + Sync>>>,
}

impl AnyConfig {
    pub fn new(value: Value) -> Self {
        Self {
            raw: Arc::new(value),
            decoded: None,
        }
    }

    pub(crate) fn get<C: 'static>(&self) -> Option<&Arc<C>> {
        self.decoded.as_ref().and_then(|local| local.downcast_ref())
    }

    pub(crate) fn decode<C>(&self) -> Result<AnyConfig, String>
    where
        C: for<'de> Deserialize<'de> + Send + Sync + 'static,
    {
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
}

impl Default for AnyConfig {
    fn default() -> Self {
        Self::new(Value::Map(Default::default()))
    }
}

impl fmt::Debug for AnyConfig {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Configs can contain credentials, so we should never print unknown configs.
        f.write_str("..")
    }
}
