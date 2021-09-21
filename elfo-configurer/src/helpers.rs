use fxhash::FxHashMap;
use serde_value::Value;

pub(crate) fn get_config(configs: &FxHashMap<String, Value>, path: &str) -> Option<Value> {
    let mut parts_iter = path.split('.');
    let mut node = configs.get(parts_iter.next()?)?;
    for part in parts_iter {
        node = if let Value::Map(map) = node {
            map.get(&Value::String(part.to_owned()))?
        } else {
            return None;
        };
    }
    Some(node.clone())
}

#[cfg(test)]
mod test {
    use super::*;

    use std::collections::BTreeMap;

    #[test]
    fn get_config_should_get_config_by_key() {
        assert_eq!(get_config(&create_configs(), "alpha"), Some(alpha_value()));
    }

    #[test]
    fn get_config_should_get_default_for_missing_key() {
        assert_eq!(get_config(&create_configs(), "beta"), None);
    }

    #[test]
    fn get_config_should_get_default_for_completely_missing_path() {
        assert_eq!(get_config(&create_configs(), "beta.beta.gamma"), None);
    }

    #[test]
    fn get_config_should_get_default_for_partially_missing_path() {
        assert_eq!(get_config(&create_configs(), "gamma.zeta.beta"), None);
        assert_eq!(get_config(&create_configs(), "alpha.zeta.beta"), None);
        assert_eq!(get_config(&create_configs(), "gamma.zeta.theta.beta"), None);
    }

    #[test]
    fn get_config_should_get_config_by_path() {
        assert_eq!(
            get_config(&create_configs(), "gamma.zeta.theta"),
            Some(theta_value())
        );
    }

    /// ```json
    /// {
    ///     "alpha": "beta",
    ///     "gamma": {
    ///         "zeta": { "theta": "iota" }
    ///     }
    /// }
    /// ```
    fn create_configs() -> FxHashMap<String, Value> {
        let mut zeta_value: BTreeMap<Value, Value> = Default::default();
        zeta_value.insert(Value::String("theta".to_owned()), theta_value());
        let zeta_value = Value::Map(zeta_value);
        let mut gamma_value: BTreeMap<Value, Value> = Default::default();
        gamma_value.insert(Value::String("zeta".to_owned()), zeta_value);
        let gamma_value = Value::Map(gamma_value);
        let mut configs: FxHashMap<String, Value> = Default::default();
        configs.insert("alpha".to_owned(), alpha_value());
        configs.insert("gamma".to_owned(), gamma_value);
        configs
    }

    fn alpha_value() -> Value {
        Value::String("beta".to_owned())
    }

    fn theta_value() -> Value {
        Value::String("iota".to_owned())
    }
}
