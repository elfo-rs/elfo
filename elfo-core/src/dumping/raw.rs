use std::{borrow::Cow, convert::AsRef};

use serde::{Serialize, Serializer};
use serde_json::value::RawValue;

/// A wrapper around a raw dump (e.g. received from WS/HTTP).
///
/// If the dump is valid JSON, it will be serialized as inner JSON
/// with saving formatting, but without newlines (replaced with a space).
///
/// Otherwise, the dump is serialized as JSON string.
#[derive(Debug)]
#[stability::unstable]
pub struct Raw<T>(pub T);

impl<T: AsRef<str>> Serialize for Raw<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = self.0.as_ref();
        let r = replace_newline(s);

        if let Some(value) = as_raw_json(&r) {
            value.serialize(serializer)
        } else {
            // Will be serialized as a JSON string with escaping.
            s.serialize(serializer)
        }
    }
}

fn replace_newline(raw: &str) -> Cow<'_, str> {
    if raw.contains('\n') {
        Cow::from(raw.replace('\n', " "))
    } else {
        Cow::from(raw)
    }
}

fn as_raw_json(raw: &str) -> Option<&RawValue> {
    let raw = raw.trim();
    serde_json::from_str(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Data {
        a: u32,
        r: Raw<String>,
    }

    #[test]
    fn json() {
        assert_eq!(
            serde_json::to_string(&Data {
                a: 42,
                r: Raw(r#"{"b":42}"#.into()),
            })
            .unwrap(),
            r#"{"a":42,"r":{"b":42}}"#
        );

        assert_eq!(
            serde_json::to_string(&Data {
                a: 42,
                r: Raw("{\"b\":\n42}".into()),
            })
            .unwrap(),
            r#"{"a":42,"r":{"b": 42}}"#
        );
    }

    #[test]
    fn non_json() {
        assert_eq!(
            serde_json::to_string(&Data {
                a: 42,
                r: Raw("foo\nbar".into()),
            })
            .unwrap(),
            r#"{"a":42,"r":"foo\nbar"}"#
        );
    }
}
