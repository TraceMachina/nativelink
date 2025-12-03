use std::{collections::BTreeSet, env, fs};
use toml::{Table, Value, map::Map};

#[derive(PartialEq, PartialOrd, Clone)]
enum LintLevel {
    Allow,
    Deny,
    Warn
}

impl LintLevel {
    fn new_from_str(value: &str) -> LintLevel {
        match value {
            "deny" => LintLevel::Deny,
            "allow" => LintLevel::Allow,
            "warn" => LintLevel::Warn,
            other => {
                panic!("Unknown level: {other}");
            }
        }
    }
}

#[derive(PartialEq)]
struct Lint {
    key: String,
    level: LintLevel,
    priority: i8,
}

impl Ord for Lint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then(self.key.cmp(&other.key))
    }
}

impl PartialOrd for Lint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for Lint {}

fn get_lints_from_key(lints_table: &Map<String, Value>, key: &str) -> BTreeSet<Lint> {
    let mut lints = BTreeSet::new();
    for (key, value) in lints_table[key]
        .as_table()
        .expect(&format!("{key} key"))
        .iter()
    {
        if let Some(v_str) = value.as_str() {
            lints.insert(Lint {
                key: key.clone(),
                level: LintLevel::new_from_str(v_str),
                priority: 0,
            });
        } else if value.is_table() {
            let table = value.as_table().expect("table");
            lints.insert(Lint {
                key: key.clone(),
                level: LintLevel::new_from_str(
                    table["level"].as_str().expect("expected level in table"),
                ),
                priority: i8::try_from(table.get("priority").and_then(|p| p.as_integer()).unwrap_or(0))
                    .expect("Expected small integer for priority"),
            });
        } else {
            panic!("{}", value);
        }
    }
    lints
}

fn generate_lint_text(flag_key: &str, lints: BTreeSet<Lint>) -> String {
    let mut rules: Vec<String> = vec![];
    for value in lints.iter() {
        let prefix = match value.level {
            LintLevel::Deny => "D",
            LintLevel::Allow => "A",
            // TODO(palfrey): these should be deny's as Bazel swallows warnings the first time it runs
            // But we've got them as warnings currently as this breaks builds as the scheduler
            // borks in some scenarios with build failures which we should fix properly later.
            LintLevel::Warn => "W",
        };
        let bazel_key = value.key.replace("-", "_");
        rules.push(format!(
            "build --@rules_rust//:{flag_key}=-{prefix}{bazel_key}"
        ));
    }
    rules.join("\n")
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        panic!(
            "Expected args: Cargo.toml and bazelrc path, got: {:?}",
            args.iter().skip(1).collect::<Vec<&String>>()
        );
    }
    let cargo_toml = &args[1];
    let bazelrc = &args[2];
    let contents = fs::read_to_string(cargo_toml).unwrap();
    let value = contents.parse::<Table>().unwrap();

    let lints_table = value["workspace"].as_table().expect("workspace key")["lints"]
        .as_table()
        .expect("lints");
    let rust_lints = get_lints_from_key(lints_table, "rust");
    let clippy_unprefixed_lints = get_lints_from_key(lints_table, "clippy");
    let mut clippy_lints = BTreeSet::new();
    for value in clippy_unprefixed_lints.iter() {
        clippy_lints.insert(Lint {
            key: format!("clippy::{}", value.key),
            level: value.level.clone(),
            priority: value.priority,
        });
    }

    let existing_bazelrc = fs::read_to_string(bazelrc).unwrap();

    let begin_cargo_lint_pattern = "## BEGIN CARGO LINTS ##";
    let end_cargo_link_pattern = "## END CARGO LINTS ##";

    let start_cargo_lint_index = existing_bazelrc
        .find(begin_cargo_lint_pattern)
        .expect(begin_cargo_lint_pattern)
        + begin_cargo_lint_pattern.len();
    let end_cargo_lint_index = existing_bazelrc
        .find(end_cargo_link_pattern)
        .expect(begin_cargo_lint_pattern);
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
