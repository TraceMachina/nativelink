use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};
use tracing::warn;

use crate::cas_server::{ByteStreamConfig, OldByteStreamConfig, WithInstanceName};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WithInstanceNameBackCompat<T> {
    Map(HashMap<String, T>),
    Vec(Vec<WithInstanceName<T>>),
}

fn deprecated(old_map: &String, new_map: &String) {
    warn!(
        r"WARNING: Using deprecated map format for services. Please migrate to the new array format:
// Old:
{}
// New:
{}
",
        old_map, new_map
    );
}

/// Use `#[serde(default, deserialize_with = "backcompat::opt_vec_with_instance_name")]` for backwards
/// compatibility with map-based access. A deprecation warning will be written to stderr if the
/// old format is used.
pub(crate) fn opt_vec_with_instance_name<'de, D, T>(
    deserializer: D,
) -> Result<Option<Vec<WithInstanceName<T>>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Serialize,
{
    let Some(back_compat) = Option::deserialize(deserializer)? else {
        return Ok(None);
    };

    match back_compat {
        WithInstanceNameBackCompat::Map(map) => {
            // TODO(palfrey): ideally this would be serde_json5::to_string_pretty but that doesn't exist
            // JSON is close enough to be workable for now
            let serde_map = serde_json::to_string_pretty(&map).expect("valid map");
            let vec: Vec<WithInstanceName<T>> = map
                .into_iter()
                .map(|(instance_name, config)| WithInstanceName {
                    instance_name,
                    config,
                })
                .collect();
            deprecated(
                &serde_map,
                // TODO(palfrey): ideally this would be serde_json5::to_string_pretty but that doesn't exist
                // JSON is close enough to be workable for now
                &serde_json::to_string_pretty(&vec).expect("valid new map"),
            );
            Ok(Some(vec))
        }
        WithInstanceNameBackCompat::Vec(vec) => Ok(Some(vec)),
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ByteStreamKind {
    Old(OldByteStreamConfig),
    New(Vec<WithInstanceName<ByteStreamConfig>>),
}

/// Use `#[serde(default, deserialize_with = "backcompat::opt_bytestream")]` for backwards
/// compatibility with older bytestream config . A deprecation warning will be written to stderr if the
/// old format is used.
pub(crate) fn opt_bytestream<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<WithInstanceName<ByteStreamConfig>>>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(back_compat) = Option::deserialize(deserializer)? else {
        return Ok(None);
    };

    match back_compat {
        ByteStreamKind::Old(old_config) => {
            if old_config.max_decoding_message_size != 0 {
                warn!(
                    "WARNING: max_decoding_message_size is ignored on Bytestream now. Please set on the HTTP listener instead"
                );
            }
            // TODO(palfrey): ideally this would be serde_json5::to_string_pretty but that doesn't exist
            // JSON is close enough to be workable for now
            let serde_map = serde_json::to_string_pretty(&old_config).expect("valid map");
            let names = old_config.cas_stores;
            let vec: Vec<WithInstanceName<_>> = names
                .iter()
                .map(|(instance_name, cas_store)| WithInstanceName {
                    instance_name: instance_name.clone(),
                    config: ByteStreamConfig {
                        cas_store: cas_store.clone(),
                        max_bytes_per_stream: old_config.max_bytes_per_stream,
                        persist_stream_on_disconnect_timeout: old_config
                            .persist_stream_on_disconnect_timeout,
                    },
                })
                .collect();
            deprecated(
                &serde_map,
                // TODO(palfrey): ideally this would be serde_json5::to_string_pretty but that doesn't exist
                // JSON is close enough to be workable for now
                &serde_json::to_string_pretty(&vec).expect("valid new map"),
            );
            Ok(Some(vec))
        }
        ByteStreamKind::New(vec) => Ok(Some(vec)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tracing_test::traced_test;

    use super::*;

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct PartialConfig {
        store: String,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct FullConfig {
        #[serde(default, deserialize_with = "opt_vec_with_instance_name")]
        cas: Option<Vec<WithInstanceName<PartialConfig>>>,
    }

    #[test]
    #[traced_test]
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

        logs_assert(|lines: &[&str]| {
            if lines.len() != 1 {
                return Err(format!("Expected 1 log line, got: {lines:?}"));
            }
            let line = lines[0];
            // TODO(palfrey): we should be checking the whole thing, but tracing-test is broken with multi-line items
            // See https://github.com/dbrgn/tracing-test/issues/48
            assert!(line.ends_with("WARNING: Using deprecated map format for services. Please migrate to the new array format:"));
            Ok(())
        });
    }

    #[test]
    fn test_deserialize_none() {
        let json = json!({});

        let value: FullConfig = serde_json::from_value(json).unwrap();
        assert_eq!(value.cas, None);
    }
}
