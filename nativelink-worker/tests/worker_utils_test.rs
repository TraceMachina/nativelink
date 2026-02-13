#![cfg(target_family = "unix")]
use std::collections::HashMap;
use std::env;

use nativelink_config::cas_server::WorkerProperty;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::platform::Property;
use nativelink_worker::worker_utils::make_connect_worker_request;

#[nativelink_test]
async fn make_connect_worker_request_with_extra_envs() -> Result<(), Error> {
    let mut worker_properties: HashMap<String, WorkerProperty> = HashMap::new();
    worker_properties.insert(
        "test".into(),
        WorkerProperty::QueryCmd("bash -c \"echo $DEMO_ENV\"".to_string()),
    );
    let mut extra_envs = HashMap::new();
    extra_envs.insert("DEMO_ENV".into(), "test_value_for_demo_env".into());

    // So we have bash for nix cases, because the PATH gets reset
    extra_envs.insert("PATH".into(), env::var("PATH").unwrap());

    let res =
        make_connect_worker_request("1234".to_string(), &worker_properties, &extra_envs, 1).await?;
    assert_eq!(
        res.properties.first(),
        Some(&Property {
            name: "test".into(),
            value: "test_value_for_demo_env".into()
        })
    );
    Ok(())
}
