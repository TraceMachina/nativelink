use std::path::PathBuf;
use std::vec::Vec;

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
    let paths = matches.get_many::<String>("inputs").unwrap().collect::<Vec<&String>>();
    let output_dir = PathBuf::from(matches.get_one::<String>("output_dir").unwrap());

    let mut config = Config::new();
    config.bytes(["."]);
    tonic_build::configure()
        .out_dir(output_dir)
        .compile_with_config(config, &paths, &["nativelink-proto"])?;
    Ok(())
}
