use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

use crate::cas_server::WithInstanceName;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WithInstanceNameBackCompat<T> {
    Map(HashMap<String, T>),
    Vec(Vec<WithInstanceName<T>>),
}

const DEPRECATION_MESSAGE: &str = r#"
WARNING: Using deprecated map format for services. Please migrate to the new array format:
    // Old:
    "cas": {
        "main": {
            "cas_store": "STORE_NAME"
        }
    }
    // New:
    "cas": [
        {
            "instance_name": "main",
            "cas_store": "STORE_NAME"
        }
    ]
"#;

/// Use `#[serde(default, deserialize_with = "backcompat::opt_vec_named_config")]` for backwards
/// compatibility with map-based access. A deprecation warning will be written to stderr if the
/// old format is used.
pub(crate) fn opt_vec_with_instance_name<'de, D, T>(
    deserializer: D,
) -> Result<Option<Vec<WithInstanceName<T>>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let Some(back_compat) = Option::deserialize(deserializer)? else {
        return Ok(None);
    };

    match back_compat {
        WithInstanceNameBackCompat::Map(map) => {
            eprintln!("{DEPRECATION_MESSAGE}");
            let vec = map
                .into_iter()
                .map(|(instance_name, config)| WithInstanceName {
                    instance_name,
                    config,
                })
                .collect();
            Ok(Some(vec))
        }
        WithInstanceNameBackCompat::Vec(vec) => Ok(Some(vec)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct PartialConfig {
        store: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct FullConfig {
        #[serde(default, deserialize_with = "opt_vec_with_instance_name")]
        cas: Option<Vec<WithInstanceName<PartialConfig>>>,
    }

    #[test]
    fn test_configs_deserialization() {
        let old_format = json!({
            "cas": {
                "foo": { "store": "foo_store" },
                "bar": { "store": "bar_store" }
            }
        });

        let new_format = json!({
            "cas": [
                {
                    "instance_name": "foo",
                    "store": "foo_store"
                },
                {
                    "instance_name": "bar",
                    "store": "bar_store"
                }
            ]
        });

        let mut old_format: FullConfig = serde_json::from_value(old_format).unwrap();
        let mut new_format: FullConfig = serde_json::from_value(new_format).unwrap();

        // Ensure deterministic ordering.
        if let Some(vec) = old_format.cas.as_mut() {
            vec.sort_by(|a, b| a.instance_name.cmp(&b.instance_name));
        }
        if let Some(vec) = new_format.cas.as_mut() {
            vec.sort_by(|a, b| a.instance_name.cmp(&b.instance_name));
        }

        assert_eq!(old_format, new_format);
    }

    #[test]
    fn test_deserialize_none() {
        let json = json!({});

        let value: FullConfig = serde_json::from_value(json).unwrap();
        assert_eq!(value.cas, None);
    }
}
