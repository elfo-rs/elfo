use serde_value::Value;

pub(crate) fn format_toml_error(content: &str, mut error: toml::de::Error) -> String {
    // `Display` would include the offending source line, which may contain
    // secrets, so only the message and a computed location are kept.
    error.set_input(None);
    let location = error.span().map_or(String::new(), |span| {
        let prefix = &content[..span.start];
        let line = prefix.matches('\n').count() + 1;
        let last_line = prefix.rsplit_once('\n').map_or(prefix, |(_, last)| last);
        let column = last_line.chars().count() + 1;
        format!(" at {line}:{column}")
    });

    format!("TOML parse error{location}: {}", error.message())
}

pub(crate) fn lookup_value<'a>(mut value: &'a Value, path: &str) -> Option<&'a Value> {
    for part in path.split('.') {
        match value {
            Value::Map(map) => value = map.get(&Value::String(part.to_owned()))?,
            _ => return None,
        }
    }

    Some(value)
}

pub(crate) fn add_defaults(config: Option<Value>, default: &Value) -> Value {
    use Value::*;

    match (config, default) {
        (None, d) => d.clone(),
        (Some(Newtype(t)), d) => add_defaults(Some(*t), d),
        (Some(t), Newtype(d)) => add_defaults(Some(t), d),
        (Some(Option(t)), d) => add_defaults(t.map(|t| *t), d),
        (Some(Map(mut config)), Map(default)) => {
            for (k, d) in default {
                let merged = add_defaults(config.remove(k), d);
                config.insert(k.clone(), merged);
            }
            Map(config)
        }
        (Some(v), _) => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> Value {
        let toml_value: Value = toml::from_str(
            r#"
            [foo]
            alpha = "beta"
            [foo.gamma]
            zeta = { theta = "iota" }
            "#,
        )
        .unwrap();

        let json_value: Value = serde_json::from_str(
            r#"
            {
                "foo": {
                    "alpha": "beta",
                    "gamma": {
                        "zeta": { "theta": "iota" }
                    }
                }
            }
            "#,
        )
        .unwrap();

        assert_eq!(toml_value, json_value);
        toml_value
    }

    fn alpha_value() -> Value {
        Value::String("beta".to_owned())
    }

    fn theta_value() -> Value {
        Value::String("iota".to_owned())
    }

    fn toml_error(content: &str) -> String {
        format_toml_error(content, toml::from_str::<Value>(content).unwrap_err())
    }

    #[test]
    fn toml_error_counts_unicode_columns() {
        let error = toml_error("value = \"пароль\" @");
        assert!(error.starts_with("TOML parse error at 1:18: "), "{error}");
        assert!(!error.contains("пароль"), "{error}");
    }

    #[test]
    fn toml_error_at_end_of_file_keeps_location() {
        let error = toml_error("[database]\npassword =");
        assert!(error.starts_with("TOML parse error at 2:11: "), "{error}");
    }

    #[test]
    fn lookup_existing_key() {
        let config = sample_config();

        assert!(matches!(lookup_value(&config, "foo"), Some(Value::Map(_))));
        assert_eq!(lookup_value(&config, "foo.alpha"), Some(&alpha_value()));
        assert_eq!(
            lookup_value(&config, "foo.gamma.zeta.theta"),
            Some(&theta_value())
        );
    }

    #[test]
    fn lookup_missing_key() {
        assert_eq!(lookup_value(&sample_config(), "foo.beta"), None);
        assert_eq!(lookup_value(&sample_config(), "foo.beta.beta.gamma"), None);
        assert_eq!(lookup_value(&sample_config(), "foo.gamma.zeta.beta"), None);
        assert_eq!(lookup_value(&sample_config(), "foo.alpha.zeta.beta"), None);
        assert_eq!(
            lookup_value(&sample_config(), "foo.gamma.zeta.theta.beta"),
            None
        );
    }

    #[test]
    fn adds_simple_defaults() {
        use Value::*;

        assert_eq!(add_defaults(None, &alpha_value()), alpha_value());
        assert_eq!(
            add_defaults(Some(Option(None)), &alpha_value()),
            alpha_value()
        );
        assert_eq!(
            add_defaults(Some(theta_value()), &alpha_value()),
            theta_value()
        );
        assert_eq!(
            add_defaults(Some(Newtype(Option(None).into())), &alpha_value()),
            alpha_value()
        );
        assert_eq!(
            add_defaults(
                Some(Newtype(Option(None).into())),
                &Newtype(alpha_value().into())
            ),
            alpha_value()
        );
        assert_eq!(
            add_defaults(
                Some(Newtype(Option(None).into())),
                &Newtype(alpha_value().into())
            ),
            alpha_value()
        );

        let config = sample_config();
        let gamma = lookup_value(&config, "foo.gamma");
        let zeta = lookup_value(&config, "foo.gamma.zeta").unwrap();

        if let Value::Map(mut map) = add_defaults(gamma.cloned(), zeta) {
            assert_eq!(map.remove(&String("theta".into())), Some(theta_value()));
            assert_eq!(map.remove(&String("zeta".into())), Some(zeta.clone()));
        } else {
            unreachable!();
        }
    }

    #[test]
    fn common_section() {
        let config: Value = toml::from_str(
            r#"
            [common.a.b.c]
            d.e = "CD"
            k.e = "CK"
            [foo]
            a.b.c.g.e = "G"
            "#,
        )
        .unwrap();

        let common = lookup_value(&config, "common").unwrap();
        let foo_config = lookup_value(&config, "foo").cloned();
        let foo_config = add_defaults(foo_config, common);

        assert_eq!(
            lookup_value(&foo_config, "a.b.c.d.e"),
            Some(&Value::String("CD".into()))
        );
        assert_eq!(
            lookup_value(&foo_config, "a.b.c.g.e"),
            Some(&Value::String("G".into()))
        );
    }

    /// The `toml` crate prior to v0.6 merges sections incorrectly,
    /// now it should work fine. Added to prevent regression.
    /// See #30 for details.
    #[test]
    fn toml_merges_sections() {
        let config: Value = toml::from_str(
            r#"
            [section]
            a.b.c = "A"
            [section.a.b.d]
            e.c = "B"
            "#,
        )
        .unwrap();

        assert_eq!(
            lookup_value(&config, "section.a.b.c"),
            Some(&Value::String("A".into()))
        );
        assert_eq!(
            lookup_value(&config, "section.a.b.d.e.c"),
            Some(&Value::String("B".into()))
        );
    }
}
