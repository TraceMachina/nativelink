//! ```sh
//! cargo run --bin build-schema --features dev-schema --package nativelink-config
//! ```

#[cfg(feature = "dev-schema")]
fn main() {
    use std::fs::File;

    use nativelink_config::cas_server::CasConfig;
    use schemars::schema_for;
    use serde_json::to_writer_pretty;
    const FILE: &str = "nativelink_config.schema.json";

    let schema = schema_for!(CasConfig);
    to_writer_pretty(File::create(FILE).expect("to create file"), &schema)
        .expect("to export schema");

    println!("Wrote schema to {FILE}");
}

#[cfg(not(feature = "dev-schema"))]
fn main() {
    eprintln!("Enable with --features dev-schema");
}
