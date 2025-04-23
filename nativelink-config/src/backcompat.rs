use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

use crate::cas_server::NamedConfig;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NamedConfigBackCompat<T> {
    Map(HashMap<String, T>),
    Vec(Vec<NamedConfig<T>>),
}

/// Use `#[serde(deserialize_with = "backcompat::opt_vec_named_config")]` for backwards
/// compatibility with map-based access. A deprecation warning will be written to stderr if the
/// old format is used.
pub(crate) fn opt_vec_named_config<'de, D, T>(
    deserializer: D,
) -> Result<Option<Vec<NamedConfig<T>>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let Some(back_compat) = Option::deserialize(deserializer)? else {
        return Ok(None);
    };

    match back_compat {
        NamedConfigBackCompat::Map(map) => {
            eprintln!(
                r#"
WARNING: Using deprecated map format for configuration. Please migrate to the new array format:
    // Old:
    "stores": {{
    "SOMESTORE": {{
        "memory": {{}}
    }}
    }}
    // New:
    "stores": [
    {{
        "name": "SOMESTORE",
        "memory": {{}}
    }}
    ]
"#
            );

            let vec = map
                .into_iter()
                .map(|(name, spec)| NamedConfig { name, spec })
                .collect();
            Ok(Some(vec))
        }
        NamedConfigBackCompat::Vec(vec) => Ok(Some(vec)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct PartialConfig {
        memory: HashMap<String, String>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct FullConfig {
        #[serde(deserialize_with = "opt_vec_named_config")]
        config: Option<Vec<NamedConfig<PartialConfig>>>,
    }

    #[test]
    fn test_configs_deserialization() {
        let old_format = json!({
            "config": {
                "store1": { "memory": {} },
                "store2": { "memory": {} }
            }
        });

        let new_format = json!({
            "config": [
                {
                    "name": "store1",
                    "memory": {}
                },
                {
                    "name": "store2",
                    "memory": {}
                }
            ]
        });

        let mut old_format: FullConfig = serde_json::from_value(old_format).unwrap();
        let mut new_format: FullConfig = serde_json::from_value(new_format).unwrap();

        // Ensure deterministic ordering.
        if let Some(vec) = old_format.config.as_mut() {
            vec.sort_by(|a, b| a.name.cmp(&b.name));
        }
        if let Some(vec) = new_format.config.as_mut() {
            vec.sort_by(|a, b| a.name.cmp(&b.name));
        }

        assert_eq!(old_format, new_format);
    }
}
