#![cfg(target_family = "unix")]
use std::collections::HashMap;
use std::env;

use nativelink_config::cas_server::WorkerProperty;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::platform::Property;
use nativelink_worker::worker_utils::make_connect_worker_request;
use tonic::Code;

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

#[nativelink_test]
async fn make_connect_worker_request_with_values() -> Result<(), Error> {
    let mut worker_properties: HashMap<String, WorkerProperty> = HashMap::new();
    worker_properties.insert("test".into(), WorkerProperty::Values(vec!["bar".into()]));

    let res =
        make_connect_worker_request("1234".to_string(), &worker_properties, &HashMap::new(), 1)
            .await?;
    assert_eq!(
        res.properties.first(),
        Some(&Property {
            name: "test".into(),
            value: "bar".into()
        })
    );
    Ok(())
}

#[nativelink_test]
async fn make_connect_worker_request_with_bad_cmd() -> Result<(), Error> {
    let mut worker_properties: HashMap<String, WorkerProperty> = HashMap::new();
    worker_properties.insert("test".into(), WorkerProperty::QueryCmd("'\\".to_string()));
    let res =
        make_connect_worker_request("1234".to_string(), &worker_properties, &HashMap::new(), 1)
            .await
            .unwrap_err();
    assert_eq!(
        res,
        Error::new(
            Code::InvalidArgument,
            "Could not parse the value of worker property: test: ''\\'".into()
        )
    );
    Ok(())
}

#[nativelink_test]
async fn make_connect_worker_request_with_bad_exit_code() -> Result<(), Error> {
    let mut worker_properties: HashMap<String, WorkerProperty> = HashMap::new();
    worker_properties.insert(
        "test".into(),
        WorkerProperty::QueryCmd("bash -c \"exit 1\"".to_string()),
    );

    let mut extra_envs = HashMap::new();
    // So we have bash for nix cases, because the PATH gets reset
    extra_envs.insert("PATH".into(), env::var("PATH").unwrap());

    let res = make_connect_worker_request("1234".to_string(), &worker_properties, &extra_envs, 1)
        .await
        .unwrap_err();
    assert_eq!(
        res,
        Error::new(
            Code::Cancelled,
            "Error executing property_name test command: 'bash -c \"exit 1\"'".into()
        )
    );
    Ok(())
}

#[nativelink_test]
async fn make_connect_worker_request_with_stderr() -> Result<(), Error> {
    let mut worker_properties: HashMap<String, WorkerProperty> = HashMap::new();
    worker_properties.insert(
        "test".into(),
        WorkerProperty::QueryCmd("bash -c \"echo foo >&2\"".to_string()),
    );

    let mut extra_envs = HashMap::new();
    // So we have bash for nix cases, because the PATH gets reset
    extra_envs.insert("PATH".into(), env::var("PATH").unwrap());

    let res =
        make_connect_worker_request("1234".to_string(), &worker_properties, &extra_envs, 1).await?;
    assert_eq!(res.properties.first(), None);
    assert!(logs_contain(
        "Got stderr when running query cmd stderr=\"foo\\n\" cmd=\"bash -c \\\"echo foo >&2\\\"\" property_name=\"test\""
    ));
    Ok(())
}

#[nativelink_test]
async fn make_connect_worker_request_with_non_utf8_stderr() -> Result<(), Error> {
    let mut worker_properties: HashMap<String, WorkerProperty> = HashMap::new();
    worker_properties.insert(
        "test".into(),
        WorkerProperty::QueryCmd("bash -c \"echo -e '\\xf0\\x20' >&2\"".to_string()),
    );

    let mut extra_envs = HashMap::new();
    // So we have bash for nix cases, because the PATH gets reset
    extra_envs.insert("PATH".into(), env::var("PATH").unwrap());

    let res =
        make_connect_worker_request("1234".to_string(), &worker_properties, &extra_envs, 1).await?;
    assert_eq!(res.properties.first(), None);
    assert!(logs_contain(
        "Got stderr when running query cmd stderr=\"� \\n\" cmd=\"bash -c \\\"echo -e '\\\\xf0\\\\x20' >&2\\\"\" property_name=\"test\""
    ));
    Ok(())
}

#[nativelink_test]
async fn make_connect_worker_request_with_multiline() -> Result<(), Error> {
    let mut worker_properties: HashMap<String, WorkerProperty> = HashMap::new();
    worker_properties.insert(
        "test".into(),
        WorkerProperty::QueryCmd("bash -c \"echo 'foo\nbar'\"".to_string()),
    );

    let mut extra_envs = HashMap::new();
    // So we have bash for nix cases, because the PATH gets reset
    extra_envs.insert("PATH".into(), env::var("PATH").unwrap());

    let res =
        make_connect_worker_request("1234".to_string(), &worker_properties, &extra_envs, 1).await?;
    assert_eq!(
        res.properties,
        vec![
            Property {
                name: "test".into(),
                value: "foo".into()
            },
            Property {
                name: "test".into(),
                value: "bar".into()
            }
        ]
    );
    Ok(())
}
