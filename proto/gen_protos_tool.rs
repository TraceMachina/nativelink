use clap::{App, Arg};
use std::path::PathBuf;
use std::vec::Vec;
use tonic_build;

fn main() -> std::io::Result<()> {
    let matches = App::new("Rust gRPC Codegen")
        .about("Codegen grpc/protobuf bindings for rust")
        .arg(
            Arg::with_name("input")
                .short("i")
                .long("input")
                .required(true)
                .multiple(true)
                .takes_value(true)
                .help("Input proto file"),
        )
        .arg(
            Arg::with_name("output_dir")
                .short("o")
                .required(true)
                .long("output_dir")
                .takes_value(true)
                .help("Output directory"),
        )
        .get_matches();
    let paths = matches.values_of("input").unwrap().collect::<Vec<&str>>();
    let output_dir = PathBuf::from(matches.value_of("output_dir").unwrap());

    tonic_build::configure()
        .out_dir(&output_dir)
        .format(true) // don't run `rustfmt`; shouldn't be needed to build
        .compile(&paths, &["proto"])?;
    Ok(())
}
