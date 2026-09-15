//! Contains useful utilities for working with configuration.
//! [The Actoromicon](https://actoromicon.rs/ch03-02-configuration.html).
//!
//! Also contains [`system`] to describe system configuration.

use std::{
    any::{Any, TypeId, type_name},
    fmt,
    ops::Deref,
    str::FromStr,
    sync::Arc,
};

use derive_more::From;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, de::value::Error as DeError};
use serde_value::{Value, ValueDeserializer};

use crate::{local::Local, panic};

/// Represents any user-defined config.
///
/// It's implemented automatically for any `Deserialize + Send + Sync + Debug`.
pub trait Config: for<'de> Deserialize<'de> + Send + Sync + fmt::Debug + 'static {}
impl<C> Config for C where C: for<'de> Deserialize<'de> + Send + Sync + fmt::Debug + 'static {}

assert_impl_all!((): Config);

// === AnyConfig ===

type RawConfig = Secret<Value>;

/// Holds user-defined config.
///
/// Usually not created directly outside tests sending [`ValidateConfig`] or
/// [`UpdateConfig`] messages.
///
/// Serialized like [`Secret`].
///
/// [`ValidateConfig`]: crate::messages::ValidateConfig
/// [`UpdateConfig`]: crate::messages::UpdateConfig
///
/// # Example
/// In tests it can be used in the following way:
/// ```
/// # use serde::Deserialize;
/// # use toml::toml;
/// # use elfo_core::config::AnyConfig;
/// AnyConfig::deserialize(toml! {
///     some_param = 10
/// });
/// ```
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnyConfig {
    raw: Arc<RawConfig>,
    #[serde(skip)]
    decoded: Option<Local<Decoded>>,
}

#[derive(Clone)]
struct Decoded {
    system: Arc<SystemConfig>,
    // Actually, we store `Arc<Arc<C>>` here.
    user: Arc<dyn Any + Send + Sync>,
}

impl AnyConfig {
    /// Creates `AnyConfig` from `serde_value::Value`.
    ///
    /// This method is unstable because it relies on the specific implementation
    /// using `serde_value`. `AnyConfig::deserialize` should be used instead
    /// where possible.
    #[instability::unstable]
    pub fn from_value(value: Value) -> Self {
        Self::from_raw(value.into())
    }

    fn from_raw(raw: RawConfig) -> Self {
        Self {
            raw: Arc::new(raw),
            decoded: None,
        }
    }

    pub(crate) fn get_user<C: 'static>(&self) -> &Arc<C> {
        self.decoded
            .as_ref()
            .and_then(|local| local.user.downcast_ref())
            .expect("must be decoded")
    }

    pub(crate) fn get_system(&self) -> &Arc<SystemConfig> {
        &self.decoded.as_ref().expect("must be decoded").system
    }

    pub(crate) fn decode<C: Config>(&self) -> Result<AnyConfig, String> {
        match panic::sync_catch(|| self.do_decode::<C>()) {
            Ok(Ok(config)) => Ok(config),
            Ok(Err(err)) => Err(err),
            Err(panic) => Err(panic),
        }
    }

    fn do_decode<C: Config>(&self) -> Result<AnyConfig, String> {
        let mut raw = Value::clone(&self.raw);

        let system_decoded = if let Value::Map(map) = &mut raw {
            if let Some(system_raw) = map.remove(&Value::String("system".into())) {
                let de = ValueDeserializer::<DeError>::new(system_raw);
                let config = SystemConfig::deserialize(de).map_err(|err| err.to_string())?;
                Arc::new(config)
            } else {
                Default::default()
            }
        } else {
            Default::default()
        };

        // Handle the special case of default config.
        let user_decoded = if TypeId::of::<C>() == TypeId::of::<()>() {
            Arc::new(Arc::new(())) as Arc<_>
        } else {
            let de = ValueDeserializer::<DeError>::new(raw);
            let config = C::deserialize(de).map_err(|err| err.to_string())?;
            Arc::new(Arc::new(config)) as Arc<_>
        };

        Ok(AnyConfig {
            raw: self.raw.clone(),
            decoded: Some(Local::from(Decoded {
                system: system_decoded,
                user: user_decoded,
            })),
        })
    }

    fn into_value(self) -> Value {
        Arc::unwrap_or_clone(self.raw).into_inner()
    }
}

impl Default for AnyConfig {
    fn default() -> Self {
        Self::from_value(Value::Map(Default::default()))
    }
}

impl fmt::Debug for AnyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.raw, f)
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

// === SystemConfig ===

pub mod system {
    //! System (`system.*` in TOML) configuration. [Config].
    //!
    //! Note: all types here are exported only for documentation purposes
    //! and are not subject to stable guarantees. However, the config
    //! structure (usually encoded in TOML) follows stable guarantees.
    //!
    //! [Config]: SystemConfig

    use super::*;

    pub use crate::{
        dumping::config as dumping, logging::config as logging, mailbox::config as mailbox,
        restarting::config as restart_policy, telemetry::config as telemetry,
    };

    /// The `system.*` section in configs.
    ///
    /// # Example
    /// ```toml
    /// [some_group]
    /// system.mailbox.capacity = 1000
    /// system.logging.max_level = "Warn"
    /// system.dumping.max_rate = 10_000
    /// system.telemetry.per_actor_key = true
    /// system.restart_policy.when = "Never"
    /// ```
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    pub struct SystemConfig {
        /// Mailbox configuration.
        pub mailbox: mailbox::MailboxConfig,
        /// Logging configuration.
        pub logging: logging::LoggingConfig,
        /// Dumping configuration.
        pub dumping: dumping::DumpingConfig,
        /// Telemetry configuration.
        pub telemetry: telemetry::TelemetryConfig,
        /// Restarting configuration.
        pub restart_policy: restart_policy::RestartPolicyConfig,
    }
}

pub(crate) use system::SystemConfig;

// === Secret ===

/// A secret value that is not printed in logs or debug output.
/// So, it's useful for storing sensitive data like credentials.
///
/// * `Debug` and `Display` instances prints `<secret>` instead of real value.
/// * `Deserialize` expects a real value.
/// * `Serialize` depends on the current [serde mode]:
///   * In the `Network` mode it's serialized as the real value.
///   * In the `Dumping` and `Normal` modes it's serialized as `"<secret>"`.
///
/// [serde mode]: crate::scope::with_serde_mode
///
/// # Example
/// ```
/// # use serde::Deserialize;
/// # use elfo_core::config::Secret;
/// #[derive(Deserialize)]
/// struct MyConfig {
///     credentials: Secret<String>,
/// }
/// ```
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

impl<T: FromStr> FromStr for Secret<T> {
    type Err = T::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        T::from_str(s).map(Self)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // serde errors quote the value, and here it's a secret.
        T::deserialize(deserializer).map(Self).map_err(|_| {
            de::Error::custom(format_args!(
                "invalid secret value, expected {}",
                type_name::<T>()
            ))
        })
    }
}

impl<T: Serialize> Serialize for Secret<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if crate::scope::serde_mode() != crate::scope::SerdeMode::Network {
            serializer.serialize_str("<secret>")
        } else {
            self.0.serialize(serializer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        messages::{UpdateConfig, ValidateConfig},
        scope::{SerdeMode, with_serde_mode},
    };

    #[test]
    fn config_messages_hide_raw_config_except_on_network() {
        let raw = RawConfig::deserialize(toml::toml! {
            credentials = { password = "do-not-log", replicas = ["nested-secret"] }
        })
        .unwrap();
        assert_eq!(format!("{raw:?}"), "<secret>");
        let config = AnyConfig::from_raw(raw.clone());
        let update = UpdateConfig::new(config.clone());
        let validate = ValidateConfig::new(config);

        for mode in [SerdeMode::Normal, SerdeMode::Dumping] {
            with_serde_mode(mode, || {
                assert_eq!(serde_json::to_string(&raw).unwrap(), r#""<secret>""#);
                for message in [
                    serde_json::to_value(&update).unwrap(),
                    serde_json::to_value(&validate).unwrap(),
                ] {
                    assert_eq!(message, serde_json::json!({ "config": "<secret>" }));
                }
            });
        }

        with_serde_mode(SerdeMode::Network, || {
            let serialized = serde_json::to_string(&update).unwrap();
            let restored: UpdateConfig = serde_json::from_str(&serialized).unwrap();
            assert_eq!(restored.config.into_value(), raw.into_inner());
        });
    }

    #[test]
    fn secret_decode_errors_hide_the_value() {
        #[derive(Debug, Deserialize)]
        enum Mode {
            Fast,
        }

        #[derive(Debug, Deserialize)]
        #[expect(dead_code)]
        struct Sample {
            password: Secret<u32>,
            mode: Secret<Mode>,
            port: u16,
        }

        let decode = |toml: toml::Table| {
            AnyConfig::deserialize(toml)
                .unwrap()
                .decode::<Sample>()
                .unwrap_err()
        };

        let reason = decode(toml::toml! {
            password = "hunter2"
            mode = "Fast"
            port = 5432
        });
        assert!(!reason.contains("hunter2"), "{reason}");
        assert!(reason.contains("expected u32"), "{reason}");

        let reason = decode(toml::toml! {
            password = 1
            mode = "hunter2"
            port = 5432
        });
        assert!(!reason.contains("hunter2"), "{reason}");
        assert!(reason.ends_with(type_name::<Mode>()), "{reason}");

        // Non-secret fields keep the value: it helps to fix the config.
        let reason = decode(toml::toml! {
            password = 1
            mode = "Fast"
            port = "not-a-secret"
        });
        assert!(reason.contains("not-a-secret"), "{reason}");
    }
}
