---
name: nativelink-config-protocol
description: Use when changing NativeLink JSON5 configuration, deployment examples, protobuf/service protocol surfaces, or compatibility-sensitive config behavior.
---

# NativeLink Config And Protocol Changes

Use this skill when a task touches `nativelink-config`, JSON5 examples, deployment config, protobuf definitions, generated protocol code, or service APIs.

## Config Changes

NativeLink config changes usually require more than editing one struct.

Check:

- `nativelink-config/` for config structs and serialization behavior.
- `nativelink-config/examples/` for JSON5 examples.
- `deployment-examples/`, `deploy/`, `kubernetes/`, and `templates/` for user-facing config samples.
- Tests that load example config files.

Preserve compatibility where practical:

- Add default values when old config files should continue to load.
- Avoid renaming public fields without aliases or migration notes.
- Keep comments in JSON5 examples accurate and actionable.
- Treat config validation errors as user-facing diagnostics.

## Protocol Changes

For service or proto work, inspect both the generated API surface and the implementation layer:

- `nativelink-proto/` for protobuf definitions and generated interfaces.
- `nativelink-service/` for REAPI, bytestream, BEP, worker API, and health service behavior.
- `nativelink-util/` for shared digest/proto conversion helpers.

Don't manually edit generated code unless the repository's generation workflow requires checked-in generated changes.

## Testing

Start focused:

```bash
bazel test //nativelink-config/...
bazel test //nativelink-service/...
```

Then broaden when the change crosses service boundaries:

```bash
bazel build //:nativelink
bazel test //...
```

For docs or examples:

```bash
bazel build docs
bazel build nativelink-config:docs
bazel test doctests
```

## Review Checklist

- Existing config files still load.
- New fields have defaults where needed.
- Error messages identify the invalid field or service behavior.
- Examples and templates match the implementation.
- Protocol changes are reflected in service tests and consumers.
