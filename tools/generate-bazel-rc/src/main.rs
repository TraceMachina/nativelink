use toml::{map::Map, Table, Value};
use std::{collections::BTreeMap, env, fs};

fn get_lints_from_key(lints_table: &Map<String, Value>, key: &str) -> BTreeMap<String, String> {
    let mut lints: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in lints_table[key].as_table().expect(&format!("{key} key")).iter() {
        if value.is_str() {
            lints.insert(key.clone(), value.as_str().unwrap().to_string());
        } else if value.is_table() {
            lints.insert(key.clone(), value.as_table().expect("table")["level"].as_str().unwrap().to_string());
        } else {
            panic!("{}", value);
        }
    }
    lints
}

fn generate_lint_text(flag_key: &str, lints: BTreeMap<String, String>) -> String {
    let mut rules: Vec<String> = vec![];
    for (key, value) in lints.iter() {
        let prefix = match value.as_str() {
            "warn" => "W",
            "deny" => "D",
            "allow" => "A",
            other => {
                panic!("Unknown level: {other}");
            }
        };
        let bazel_key = key.replace("-", "_");
        rules.push(format!("build --@rules_rust//:{flag_key}=-{prefix}{bazel_key}"));
    }
    rules.join("\n")
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        panic!("Expected args: Cargo.toml and bazelrc path, got: {:?}", args.iter().skip(1).collect::<Vec<&String>>());
    }
    let cargo_toml = &args[1];
    let bazelrc = &args[2];
    let contents = fs::read_to_string(cargo_toml).unwrap();
    let value = contents.parse::<Table>().unwrap();

    let lints_table= value["workspace"].as_table().expect("workspace key")["lints"].as_table().expect("lints");
    let rust_lints = get_lints_from_key(lints_table,"rust");
    let clippy_unprefixed_lints = get_lints_from_key(lints_table,"clippy");
    let mut clippy_lints = BTreeMap::new();
    for (key, value) in clippy_unprefixed_lints.iter() {
        clippy_lints.insert(format!("clippy::{key}"), value.to_string());
    }

    let existing_bazelrc = fs::read_to_string(bazelrc).unwrap();

    let begin_cargo_lint_pattern = "## BEGIN CARGO LINTS ##";
    let end_cargo_link_pattern = "## END CARGO LINTS ##";

    let start_cargo_lint_index = existing_bazelrc.find(begin_cargo_lint_pattern).expect(begin_cargo_lint_pattern) + begin_cargo_lint_pattern.len();
    let end_cargo_lint_index = existing_bazelrc.find(end_cargo_link_pattern).expect(begin_cargo_lint_pattern);
    let mut new_bazelrc = existing_bazelrc[0..start_cargo_lint_index].to_string();
    new_bazelrc.push_str("\n\n");
    new_bazelrc.push_str(&generate_lint_text("extra_rustc_flag", rust_lints));
    new_bazelrc.push_str("\n\n");
    new_bazelrc.push_str(&generate_lint_text("clippy_flag", clippy_lints));
    new_bazelrc.push_str("\n\n");
    new_bazelrc.push_str(&existing_bazelrc[end_cargo_lint_index..]);

    if new_bazelrc != existing_bazelrc {
        fs::write(bazelrc, new_bazelrc).unwrap();
    }
}
