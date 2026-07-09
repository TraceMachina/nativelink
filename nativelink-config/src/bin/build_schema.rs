//! ```sh
//! cargo run --bin build-schema --features dev-schema --package nativelink-config
//! ```

#[cfg(feature = "dev-schema")]
fn main() {
    use std::env;
    use std::fs::File;

    use nativelink_config::cas_server::CasConfig;
    use schemars::schema_for;
    use serde_json::to_writer_pretty;

    let output_file = env::args()
        .nth(1)
        .unwrap_or("nativelink_config.schema.json".into());

    let schema = schema_for!(CasConfig);
    to_writer_pretty(File::create(&output_file).expect("to create file"), &schema)
        .expect("to export schema");

    println!("Wrote schema to {output_file}");
}

#[cfg(not(feature = "dev-schema"))]
fn main() {
    eprintln!("Enable with --features dev-schema");
}
