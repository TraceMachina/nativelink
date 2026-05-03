use std::collections::HashMap;
use std::path::PathBuf;

use clap::{Arg, ArgAction, Command};
use prost_build::Config;

fn main() -> std::io::Result<()> {
    let matches = Command::new("Rust gRPC Codegen")
        .about("Codegen grpc/protobuf bindings for rust")
        .arg(
            Arg::new("inputs")
                .required(true)
                .action(ArgAction::Append)
                .help("Input proto files"),
        )
        .arg(
            Arg::new("output_dir")
                .short('o')
                .required(true)
                .long("output_dir")
                .help("Output directory"),
        )
        .get_matches();
    let paths = matches
        .get_many::<String>("inputs")
        .unwrap()
        .collect::<Vec<&String>>();
    let output_dir = PathBuf::from(matches.get_one::<String>("output_dir").unwrap());

    let mut config = Config::new();
    config.bytes(["."]);

    let mut structs_with_data_to_ignore = HashMap::new();
    structs_with_data_to_ignore.insert("BatchReadBlobsResponse.Response", vec!["data"]);
    structs_with_data_to_ignore.insert("BatchUpdateBlobsRequest.Request", vec!["data"]);
    structs_with_data_to_ignore.insert("ReadResponse", vec!["data"]);
    structs_with_data_to_ignore.insert("WriteRequest", vec!["data"]);
    structs_with_data_to_ignore.insert("ActionResult", vec!["output_files"]);

    for (struct_name, fields) in &structs_with_data_to_ignore {
        config.type_attribute(struct_name, "#[derive(::derive_more::Debug)]");
        for field in fields {
            config.field_attribute(format!("{struct_name}.{field}"), "#[debug(ignore)]");
        }
    }

    config.skip_debug(structs_with_data_to_ignore.keys());

    tonic_build::configure()
        .out_dir(output_dir)
        .compile_protos_with_config(config, &paths, &["nativelink-proto"])?;
    Ok(())
}
