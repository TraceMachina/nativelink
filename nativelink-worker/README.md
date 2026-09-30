# Buck2 capture lifecycle

`experimental_buck2_file_capture` connects a worker to an operator-supplied file
capture helper. It is disabled by default. The helper owns file collection,
storage, authorization, and retention; NativeLink coordinates its lifetime with
the action. This repository does not bundle a collector or an ingest service.

The option requires a Linux worker with `single_use: true`, running inside a
container dedicated to one action. The launcher must replace that container and
its writable volumes after each action. Entrypoint wrappers and separate action
mount namespaces are rejected because the helper would see a different
filesystem from the executing action. For nested execution, both NativeLink and
the helper must run inside the execution container.

The scheduler forwards the original REAPI request metadata with the assignment.
Only an assignment identifying `buck2` with a nonempty invocation ID starts the
helper. Tool names are case-insensitive. Missing metadata and other build tools
leave capture disabled. Request metadata identifies a build; it is not proof of
ownership. The ingest service must independently enforce tenant authorization.

For dedicated execution across overlapping builds, also enable
`experimental_buck2_invocation_isolation` on each Execution service for the
instance. It separates in-flight actions by Buck2 invocation and request
identity, even when their action digests match. Without this option, identical
actions from different builds can join an existing execution. Completed action
cache results remain shared and do not start a worker or collector. Retries
within one invocation can still join its running action. Other build tools and
requests without Buck2 invocation metadata retain ordinary deduplication.
Upgrade all schedulers serving the instance before enabling the option.

## Helper control protocol

NativeLink starts the configured executable with no arguments, an empty
environment except for optional `SSL_CERT_FILE` and `SSL_CERT_DIR`, and private
stdin/stdout pipes. Stderr is inherited for diagnostics. The first stdin line is
a JSON record with these fields:

| Field | Meaning |
| --- | --- |
| `root` | `/`, the filesystem of the execution container. |
| `stateDirectory` | Configured state directory plus a fresh session UUID. |
| `protectedPaths` | Credential and configuration paths the helper must exclude, including aliases. |
| `internalPaths` | Internal cache paths excluded from browsing; materialized action inputs remain visible. |
| `gateway` | Configured authenticated ingest endpoint. |
| `tokenFile` | Path to the helper's private credential; the JSON never contains its contents. |
| `finalizeSeconds` | Final capture budget, default 120 seconds and at most 3600. |
| `session` | `id`, `tool`, `build`, `action`, `attempt`, `worker`, and `container`. |

`session.id` is a fresh UUID; `tool` is `buck2`; `build` is the invocation ID;
`action` is the action digest hash; `attempt` is the operation ID. `worker` is
the assigned worker ID and `container` is the configured execution-container
identity. Retries get separate sessions.

The helper must register its session and write exactly `ready\n` to stdout within
30 seconds. Stdout is reserved for that acknowledgement. File bytes and
credentials must never be written to it. NativeLink then prepares and executes
the action while the helper collects files.

Before action-directory cleanup, NativeLink closes the helper's stdin. The
helper must finish its final capture and exit successfully only after durable
acknowledgement. This applies to success, failure, cancellation, and timeout.
NativeLink allows the configured finalization budget plus ten seconds for
reporting a partial capture and exiting, then kills and reaps an unresponsive
helper. The single-use worker cannot complete before this cleanup path returns.

Capture failures are logged without changing the action's execution result. A
failed or interrupted collector must leave its archive visibly partial or
unavailable. Process or node loss can prevent finalization entirely; this hook
does not recover bytes that the helper never persisted.

## Deployment boundaries

Run the helper with the least privilege needed to read permitted regular files.
Its file API must exclude platform credentials, enforce path and mount
boundaries, and authorize each session independently of client-provided request
metadata. Do not expose a reusable worker's filesystem or the host filesystem.
NativeLink does not open a browser-facing file endpoint.

Configure the helper executable, state directory, protected paths, and token
file with absolute paths. The token path is passed only to the helper, never
added to the action environment. This does not isolate credentials from an
action running with the same user privileges. Use narrowly scoped collector
credentials and enforce the required trust boundary in the execution environment.
Do not use `NATIVELINK_DONT_CLEANUP` to preserve
files: it is a debug option that also bypasses action-manager cleanup.
