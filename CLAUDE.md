# NativeLink Rust Style & Conventions

## Imports
- Order: `core::` → `std::` → external crates (alphabetical) → internal `nativelink-*` crates
- Group proto imports by module, not alphabetically
- Use `use crate::...` for same-crate modules

## Error Handling
- `make_err!(Code::..., "message")` for internal errors; `make_input_err!("message")` for bad input
- `error_if!(condition, "message")` for early validation returns
- `.err_tip(|| "context")` to chain diagnostic context onto Results
- Never panic or unwrap in library code; always return `Result<T, Error>`
- Use `{:?}` for debug formatting of upstream errors in messages

## Logging (tracing)
- `use tracing::{debug, error, info, trace, warn};`
- Structured fields: `%` for Display, `?` for Debug — `info!(%key, ?value, "message")`
- `info!` for state transitions, transfer completions with throughput/duration
- `warn!` for performance anomalies (slow ops, contention, early evictions)
- `trace!` for hot-path / repetitive loops; avoid logging inside tight loops
- Messages: lowercase, no trailing period, describe **why** not what

## Async & Concurrency
- `#[async_trait]` on trait definitions and impls
- `Pin<&Self>` for `StoreDriver` trait methods
- `spawn_blocking()` for CPU-bound or sync filesystem work; avoid async recursion
- `tokio::join!` for fixed concurrent work; `FuturesUnordered` for variable-count
- `parking_lot::Mutex` for sync contexts; never hold locks across `.await`

## Config (serde)
- `#[serde(deny_unknown_fields)]` on config structs
- `#[serde(default)]` or `#[serde(default = "fn_name")]` for optional fields
- `#[serde(deserialize_with = "convert_string_with_shellexpand")]` for paths
- `#[serde(rename_all = "snake_case")]` on enums

## Metrics
- `#[derive(MetricsComponent)]` on public structs
- `#[metric(help = "...")]` on fields; `#[metric(group = "...")]` for nesting

## Naming & Formatting
- Functions: `snake_case`; types: `PascalCase`; constants: `UPPER_SNAKE_CASE`
- ~100 char soft line limit; readability over rigid length
- Blank line between logical sections; single blank line between items
- `Cow<'_, T>` in hot paths to avoid allocation

## Comments
- `///` doc comments on public items explain **why** and show examples
- `//` inline comments only for non-obvious logic or workarounds
- `TODO(...)` with issue number when possible for known issues

## Feature Gates
- `#[cfg(feature = "...")]` at definition site
- `#[cfg(target_os = "...")]` for OS-specific code (Linux vs macOS)

## Tests
- **Test-first development**: when implementing any new feature, write tests first
  (unit, integration, and cross-component interaction tests). Verify they fail before
  implementing the feature, then make them pass. Include fakes/mocks for
  hardware-interaction tests where needed.
- Integration tests in `tests/` directory; minimal inline `#[cfg(test)]` modules
- Use `nativelink-macro` test harness (`#[nativelink_test]`)

## Change Process
- **Chesterton's Fence**: before modifying or removing any behavior, always check
  `git log`, `git blame`, and `git log -S` to understand *why* the code exists.
  If a commit message or comment explains the reason, evaluate whether that reason
  still applies before making the change.
