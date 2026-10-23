// Copyright 2026 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `nl-nix` — a `nix copy`-style client that pushes and pulls store paths
//! to/from a NativeLink `nix_cache`, streaming zstd on the wire. The `flake`
//! subcommand builds a whole flake's outputs and pushes their closures, so the
//! "build everything, upload it all" flow needs no external shell wrapper.

use std::collections::HashSet;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use futures::stream::{FuturesUnordered, StreamExt as _};
use nativelink_error::{Error, ResultExt, make_input_err};
use nativelink_nix_client::client::{CacheClient, PushOutcome};
use nativelink_nix_client::config::{Auth, CacheConfig};
use nativelink_nix_client::metrics::Metrics;
use nativelink_nix_client::store::{NixStore, StorePath};

#[derive(Parser)]
#[command(
    name = "nl-nix",
    version,
    about = "Nix binary-cache client for NativeLink: push/pull, streaming zstd, OTLP metrics"
)]
struct Cli {
    /// Base URL of the cache.
    #[arg(long, env = "NL_NIX_CACHE")]
    to: String,
    /// Local Nix store directory.
    #[arg(long, default_value = "/nix/store")]
    store: String,
    /// Bearer token (Basic uses it as the password with an empty user).
    #[arg(long, env = "NL_NIX_TOKEN")]
    token: Option<String>,
    /// Ed25519 secret-key file to sign pushed narinfo (repeatable).
    #[arg(long = "signing-key")]
    signing_keys: Vec<String>,
    /// zstd level for uploads.
    #[arg(long, default_value_t = 3)]
    compression_level: i32,
    /// Upload uncompressed instead of zstd.
    #[arg(long)]
    no_compress: bool,
    /// Maximum concurrent path transfers.
    #[arg(long, default_value_t = 8)]
    jobs: usize,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Push store paths (optionally their whole closure) to the cache.
    Push {
        /// Store paths to push.
        paths: Vec<String>,
        /// Also push the entire reference closure of each path.
        #[arg(long)]
        recursive: bool,
    },
    /// Pull store paths by 32-char hash from the cache, verifying + restoring
    /// each NAR into `--out/<hash>` (does not register them in the store).
    Pull {
        /// Store-path hashes to pull.
        hashes: Vec<String>,
        /// Directory to restore each path into.
        #[arg(long)]
        out: String,
    },
    /// Build every output of a flake for this system and push each path's whole
    /// closure. Absorbs the enumerate → `nix build` → push flow that previously
    /// needed an external shell wrapper.
    Flake {
        /// Flake reference to build (default: the flake in the current directory).
        #[arg(default_value = ".")]
        flake: String,
        /// Output types to enumerate, space- or comma-separated.
        #[arg(long, env = "NL_PUSH_OUTPUTS", default_value = "packages devShells")]
        outputs: String,
    },
    /// Reconcile every valid path in the local Nix database into the cache.
    /// Covers paths predating the watcher and every multi-output result.
    Reconcile,
    /// Check whether store paths — or, with `--recursive`, their whole
    /// closures — have usable metadata in the cache, without transferring NARs.
    /// Accepts full store paths or bare 32-char hashes. Prints `present`/`absent`
    /// per path and exits non-zero if any are absent, so it doubles as a shell
    /// predicate: `nl-nix --to … has "$P" && echo cached`.
    #[command(alias = "check")]
    Has {
        /// Store paths or 32-char nixbase32 hashes to check.
        paths: Vec<String>,
        /// Also check the entire reference closure of each local store path.
        #[arg(long)]
        recursive: bool,
        /// Suppress per-path output; rely on the exit code only.
        #[arg(long, short)]
        quiet: bool,
    },
    /// Print the cache's `/nix-cache-info`.
    Info,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Best-effort OTLP telemetry, identical to the server; a missing collector
    // just means metrics are dropped, never that the client fails.
    if let Err(err) = nativelink_util::telemetry::init_tracing().await {
        eprintln!("nl-nix: telemetry init failed (continuing without metrics): {err}");
    }
    run(Cli::parse()).await
}

async fn run(cli: Cli) -> Result<(), Error> {
    let config = CacheConfig {
        base_url: cli.to.clone(),
        auth: cli.token.clone().map(Auth::Bearer),
        signing_key_files: cli.signing_keys.clone(),
        store_dir: cli.store.clone(),
        compress: !cli.no_compress,
        compression_level: cli.compression_level,
        concurrency: cli.jobs,
        dedup: true,
    };
    let metrics = Arc::new(Metrics::new("nl-nix"));
    let client = Arc::new(CacheClient::new(config, metrics)?);

    match cli.command {
        Command::Info => {
            print!("{}", client.cache_info().await?);
            Ok(())
        }
        Command::Push { paths, recursive } => {
            let store = Arc::new(NixStore::open(&cli.store)?);
            let roots = parse_paths(&cli.store, &paths)?;
            let ordered = if recursive {
                closure(&store, roots)?
            } else {
                roots
            };
            push_all(&client, &store, ordered, cli.jobs).await
        }
        Command::Pull { hashes, out } => {
            let out = std::path::PathBuf::from(out);
            for hash in hashes {
                let dest = out.join(&hash);
                let size = client.pull_path(&hash, &[], &dest).await?;
                println!("pulled {hash} ({size} NAR bytes) -> {}", dest.display());
            }
            Ok(())
        }
        Command::Has {
            paths,
            recursive,
            quiet,
        } => has_all(&client, &cli.store, paths, recursive, quiet, cli.jobs).await,
        Command::Flake { flake, outputs } => {
            let system = nix_current_system().await?;
            eprintln!("nl-nix: flake={flake} system={system} outputs=[{outputs}]");

            // Enumerate buildable attrs, one output type at a time (a type that
            // doesn't evaluate — e.g. a flake with no devShells — is skipped,
            // not fatal).
            let mut attrs = Vec::new();
            for kind in outputs.split([' ', ',']).filter(|s| !s.is_empty()) {
                attrs.extend(flake_attrs(&flake, kind, &system).await);
            }
            if attrs.is_empty() {
                eprintln!("nl-nix: no buildable outputs for {system} in [{outputs}]");
                return Ok(());
            }
            eprintln!("nl-nix: building {} outputs", attrs.len());

            let out_paths = nix_build(&flake, &attrs).await?;
            if out_paths.is_empty() {
                return Err(make_input_err!("`nix build` produced no outputs"));
            }
            eprintln!("nl-nix: built {} paths; pushing closures", out_paths.len());

            let store = Arc::new(NixStore::open(&cli.store)?);
            let roots = out_paths
                .iter()
                .map(|p| StorePath::from_full_path(&cli.store, p))
                .collect::<Result<Vec<_>, _>>()?;
            let ordered = closure(&store, roots)?;
            push_all(&client, &store, ordered, cli.jobs).await
        }
        Command::Reconcile => {
            let store = Arc::new(NixStore::open(&cli.store)?);
            let paths = store.all_valid_paths()?;
            eprintln!("nl-nix: reconciling {} valid store paths", paths.len());
            push_all(&client, &store, paths, cli.jobs).await
        }
    }
}

/// Runs `nix` with `args`, returning its stdout and whether it exited zero.
/// `show_progress` inherits the child's stderr (so `nix build` progress reaches
/// the terminal); otherwise stderr is discarded (quiet probing).
async fn run_nix(args: &[String], show_progress: bool) -> Result<(String, bool), Error> {
    let stderr = if show_progress {
        std::process::Stdio::inherit()
    } else {
        std::process::Stdio::null()
    };
    let output = tokio::process::Command::new("nix")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(stderr)
        .output()
        .await
        .map_err(|e| make_input_err!("spawning `nix` (is it on PATH?): {e}"))?;
    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.success(),
    ))
}

/// The Nix system double for this host (`builtins.currentSystem`).
async fn nix_current_system() -> Result<String, Error> {
    let (out, ok) = run_nix(
        &[
            "eval".into(),
            "--raw".into(),
            "--impure".into(),
            "--expr".into(),
            "builtins.currentSystem".into(),
        ],
        true,
    )
    .await?;
    if !ok {
        return Err(make_input_err!("`nix eval builtins.currentSystem` failed"));
    }
    Ok(out.trim().to_string())
}

/// The full installable attrs under `<flake>#<kind>.<system>`, empty if that
/// output type is absent. Uses `--apply` to emit newline-separated names, so no
/// JSON parser is needed.
async fn flake_attrs(flake: &str, kind: &str, system: &str) -> Vec<String> {
    let args = vec![
        "eval".into(),
        "--raw".into(),
        format!("{flake}#{kind}.{system}"),
        "--apply".into(),
        "a: builtins.concatStringsSep \"\\n\" (builtins.attrNames a)".into(),
    ];
    match run_nix(&args, false).await {
        Ok((out, true)) => out
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| format!("{kind}.{system}.{l}"))
            .collect(),
        _ => Vec::new(),
    }
}

/// Builds every installable with `--keep-going` (one broken output doesn't sink
/// the rest) and returns the realised out paths. Partial failure warns but still
/// returns what built.
async fn nix_build(flake: &str, attrs: &[String]) -> Result<Vec<String>, Error> {
    let mut args = vec![
        "build".into(),
        "--no-link".into(),
        "--keep-going".into(),
        "--print-out-paths".into(),
    ];
    args.extend(attrs.iter().map(|a| format!("{flake}#{a}")));
    let (out, ok) = run_nix(&args, true).await?;
    if !ok {
        eprintln!("nl-nix: some outputs failed to build; pushing the ones that succeeded");
    }
    Ok(out
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Parses CLI path arguments into validated store paths.
fn parse_paths(store_dir: &str, paths: &[String]) -> Result<Vec<StorePath>, Error> {
    if paths.is_empty() {
        return Err(make_input_err!("no store paths given"));
    }
    paths
        .iter()
        .map(|p| StorePath::from_full_path(store_dir, p))
        .collect()
}

/// Computes the reference closure of `roots`, returned references-first so a
/// path is always pushed after everything it depends on.
fn closure(store: &NixStore, roots: Vec<StorePath>) -> Result<Vec<StorePath>, Error> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut order: Vec<StorePath> = Vec::new();
    for root in roots {
        visit(store, root, &mut seen, &mut order)?;
    }
    Ok(order)
}

fn visit(
    store: &NixStore,
    path: StorePath,
    seen: &mut HashSet<String>,
    order: &mut Vec<StorePath>,
) -> Result<(), Error> {
    if !seen.insert(path.full_path().to_string()) {
        return Ok(());
    }
    let meta = store.query_path_info(&path)?;
    for reference in meta.references {
        // A path references itself; skip to avoid infinite recursion.
        if reference.full_path() != path.full_path() {
            visit(store, reference, seen, order)?;
        }
    }
    order.push(path);
    Ok(())
}

/// Pushes every path with bounded concurrency, reporting a one-line summary.
async fn push_all(
    client: &Arc<CacheClient>,
    store: &Arc<NixStore>,
    paths: Vec<StorePath>,
    jobs: usize,
) -> Result<(), Error> {
    let total = paths.len();
    let jobs = jobs.max(1);
    let mut iter = paths.into_iter();
    let mut inflight = FuturesUnordered::new();
    let mut uploaded = 0u64;
    let mut skipped = 0u64;
    let mut wire = 0u64;

    loop {
        while inflight.len() < jobs {
            let Some(path) = iter.next() else { break };
            let client = Arc::clone(client);
            let store = Arc::clone(store);
            inflight.push(async move {
                let meta = store.query_path_info(&path)?;
                let outcome = client.push_path(&meta).await?;
                Ok::<(StorePath, PushOutcome), Error>((path, outcome))
            });
        }
        let Some(result) = inflight.next().await else {
            break;
        };
        match result {
            Ok((path, PushOutcome::Uploaded { wire_bytes, .. })) => {
                uploaded += 1;
                wire += wire_bytes;
                println!("pushed {}", path.full_path());
            }
            Ok((path, PushOutcome::AlreadyPresent)) => {
                skipped += 1;
                println!("skipped (present) {}", path.full_path());
            }
            Err(err) => {
                eprintln!("push failed: {err}");
                return Err(err).err_tip(|| "one or more pushes failed");
            }
        }
    }

    println!(
        "done: {uploaded} uploaded, {skipped} already present, of {total} paths ({wire} wire bytes)"
    );
    Ok(())
}

/// Probes the cache for each input, closure-expanded when `--recursive`, and
/// reports `present`/`absent` per path (unless `quiet`). A validated
/// `GET {hash}.narinfo` per path — the same dedup probe `push` uses — transfers
/// metadata but no NAR data.
/// Exits non-zero if any probed path is absent, making it a shell predicate.
async fn has_all(
    client: &Arc<CacheClient>,
    store_dir: &str,
    inputs: Vec<String>,
    recursive: bool,
    quiet: bool,
    jobs: usize,
) -> Result<(), Error> {
    if inputs.is_empty() {
        return Err(make_input_err!("no store paths or hashes given"));
    }

    // Resolve every input to a de-duplicated (display, hash) probe in a stable
    // order. A full store path may expand to its whole closure; a bare hash is
    // probed as-is (it has no local closure to expand).
    let store = if recursive {
        Some(NixStore::open(store_dir)?)
    } else {
        None
    };
    let mut probes: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for input in &inputs {
        if input.contains('/') {
            let root = StorePath::from_full_path(store_dir, input)?;
            let paths = match &store {
                Some(store) => closure(store, vec![root])?,
                None => vec![root],
            };
            for path in paths {
                if seen.insert(path.hash_part().to_string()) {
                    probes.push((path.full_path().to_string(), path.hash_part().to_string()));
                }
            }
        } else {
            if recursive {
                return Err(make_input_err!(
                    "--recursive needs a full store path; '{input}' is a bare hash with no local closure"
                ));
            }
            // Validate the bare hash by round-tripping it through StorePath's
            // parser (which enforces 32-char nixbase32) — no extra dependency.
            let synthetic = format!("{store_dir}/{input}-probe");
            let path = StorePath::from_full_path(store_dir, &synthetic).err_tip(|| {
                format!("'{input}' is neither a store path nor a 32-char nixbase32 hash")
            })?;
            if seen.insert(path.hash_part().to_string()) {
                probes.push((input.clone(), path.hash_part().to_string()));
            }
        }
    }

    // Probe with bounded concurrency, preserving input order for the report.
    let jobs = jobs.max(1);
    let mut results: Vec<bool> = vec![false; probes.len()];
    let mut iter = probes.iter().enumerate();
    let mut inflight = FuturesUnordered::new();
    loop {
        while inflight.len() < jobs {
            let Some((idx, (_display, hash))) = iter.next() else {
                break;
            };
            let client = Arc::clone(client);
            let hash = hash.clone();
            inflight.push(async move { (idx, client.has_path(&hash).await) });
        }
        let Some((idx, present)) = inflight.next().await else {
            break;
        };
        results[idx] = present?;
    }

    let mut absent = 0u64;
    for (present, (display, _hash)) in results.iter().zip(&probes) {
        if !present {
            absent += 1;
        }
        if !quiet {
            println!("{} {display}", if *present { "present" } else { "absent " });
        }
    }
    let total = probes.len();
    if !quiet {
        eprintln!(
            "{}: {absent} of {total} absent",
            if absent == 0 { "ok" } else { "missing" }
        );
    }
    // Clean predicate exit: non-zero when anything is absent, without dumping a
    // Rust error (which `run` -> `main` would print). stdout is line-buffered and
    // already flushed by the per-path `println!`s above.
    if absent > 0 {
        std::process::exit(1);
    }
    Ok(())
}
