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

    let structs_with_data_to_ignore = [
        "BatchReadBlobsResponse.Response",
        "BatchUpdateBlobsRequest.Request",
        "ReadResponse",
        "WriteRequest",
    ];

    for struct_name in structs_with_data_to_ignore {
        config.type_attribute(struct_name, "#[derive(::derive_more::Debug)]");
        config.field_attribute(format!("{struct_name}.data"), "#[debug(ignore)]");
    }

    config.skip_debug(structs_with_data_to_ignore);

    tonic_build::configure()
        .out_dir(output_dir)
        .compile_protos_with_config(config, &paths, &["nativelink-proto"])?;
    Ok(())
}
