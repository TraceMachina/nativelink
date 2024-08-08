use std::env;

fn main() {
    // Retrieve Git commit hash
    let git_hash = if let Ok(output) = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .output()
    {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        "UNKNOWN".to_string()
    };

    // Retrieve version from Cargo.toml
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "UNKNOWN".to_string());

    // Write the version and hash to a file or directly include them as environment variables
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=.git");
    println!("cargo:rustc-env=NATIVELINK_APP_VERSION={}", version);
    println!("cargo:rustc-env=NATIVELINK_GIT_COMMIT_HASH={}", git_hash);
}
