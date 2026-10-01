# Sandboxing and Isolation

Actions must not observe each other's state. A test that passes because it reads a file left behind by a previous action is a correctness bug. NativeLink provides multiple isolation mechanisms, from filesystem sandboxing to full Linux namespace isolation.

## Filesystem Isolation

Every action gets its own working directory under the configured `work_directory`:

```
/data/work/
├── action-a1b2c3d4/
│   ├── src/main.c     (hardlinked from CAS)
│   ├── include/foo.h  (hardlinked from CAS)
│   └── output/        (created by action)
├── action-e5f6g7h8/
│   └── ...
```

After an action completes, its working directory is cleaned up. The next action starts with a fresh directory.

Input files are hardlinked from the worker's local CAS into the action's working directory (`running_actions_manager.rs:365-374`). Read-only is **enforced by the file mode**, not by convention and not by mount namespaces: the CAS blob is stored `0o444` (`filesystem_store.rs:1207`), and an executable input is hardlinked to a per-digest `0o555` variant that is created once, off the hot path (`get_executable_hardlink_source`), because the shared `0o444` blob cannot carry the `+x` bit. A hardlink shares the underlying inode with the CAS blob, so that mode is shared too — every action that materializes the same input points at the same read-only inode, and none of them can write to it.

This is also why the worker must never `chmod` a hardlinked input: mutating the mode (or the content) of one action's input silently corrupts the shared inode for *every* other in-flight action holding the same blob. That corruption class is the reason executables get a private `0o555` variant instead of a `chmod`, and the reason directory-cache eviction changes the mode of directories only, never of the hardlinked files (`directory_cache.rs:999-1001`). Reconstructed input trees served from the directory cache are locked down the same way — files `0o555`, directories `0o755` — by `set_readonly_recursive` (`fs_util.rs:329`) and the cache's own copy path (`directory_cache.rs:552`).

## Linux Namespace Isolation

For stronger isolation, NativeLink supports Linux namespaces:

```json5
local: {
  use_namespaces: true,       // PID, user, IPC, UTS namespaces
  use_mount_namespace: true   // mount namespace (filesystem isolation)
}
```

**Source:** [`nativelink-worker/src/namespace_utils.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-worker/src/namespace_utils.rs)

When enabled, the worker calls `unshare()` (`configure_namespace`, `namespace_utils.rs:399`) with:
- `CLONE_NEWPID` — isolated PID namespace. The action becomes **PID 1** of a fresh namespace and sees only its own process tree.
- `CLONE_NEWUSER` — user namespace. The worker writes an **identity** `uid_map` (`{uid} {uid} 1`, `namespace_utils.rs:429`), so the action keeps its own unprivileged `uid` — it is *not* remapped to root — while holding capabilities only inside its own user namespace. `setgroups` is denied and the `gid_map` is best-effort (failures are tolerated, `namespace_utils.rs:420-441`).
- `CLONE_NEWIPC` — IPC namespace (no shared System V or POSIX shared memory between actions).
- `CLONE_NEWUTS` — UTS namespace; the hostname is pinned to `nativelink` for reproducibility (`namespace_utils.rs:448-452`).
- `CLONE_NEWNS` (only if `use_mount_namespace`) — mount namespace, described below.

Deliberately **no** `CLONE_NEWNET` is set: actions in a namespace share the host network. Network isolation only comes from a container entrypoint (below).

### What the mount namespace actually does

The mount namespace does **not** bind-mount inputs read-only — read-only already comes from the `0o444`/`0o555` file modes above, with or without a mount namespace. What `CLONE_NEWNS` adds is **sibling masking**. After the `unshare`, `perform_remount` (`namespace_utils.rs:299-390`):

1. Remounts `/` as `MS_REC | MS_PRIVATE` so nothing propagates back to the host.
2. Bind-mounts the action's own directory onto itself and pins it open with an `O_PATH` file descriptor (so it survives the next step).
3. Mounts an empty `tmpfs` over the shared `root_action_directory` — this hides *every other* action's working directory in one move.
4. Recreates the action's directory inside that fresh `tmpfs` and bind-mounts the pinned original back into place.

The net effect: the action sees its own working directory and nothing else under the worker's root. Combined with the PID and IPC namespaces, an action can neither observe another action's processes and shared memory nor even name another action's files on disk.

### The reaper stub (PID 1)

Namespace isolation is **not** "just a kernel flag." After setting up the namespaces, `configure_namespace` **forks** (`namespace_utils.rs:463`):

- The **child** sets `PR_SET_PDEATHSIG = SIGKILL` (so it dies if the stub dies) and returns; the worker's `execve` then replaces it with the action binary. Because it is the first process created after `unshare(CLONE_NEWPID)`, the action runs as **PID 1** of the new PID namespace.
- The **parent** never returns. It becomes a small reaper stub in the worker's namespace: it sets `PR_SET_CHILD_SUBREAPER`, closes all inherited file descriptors, blocks `SIGTERM`/`SIGCHLD`, and loops reaping children (`namespace_utils.rs:471-533`). When the action exits it re-propagates the action's exit code, or re-raises its terminating signal; on `SIGTERM` it forwards `SIGKILL` to the action.

The stub is the process the worker's tokio `Child` actually tracks, which is why `MaybeNamespacedChild::kill` sends `SIGTERM` to the *stub* — the stub then `SIGKILL`s PID 1, tearing the whole namespace down (`namespace_utils.rs:38-53`).

**Why bother — zombie reaping.** Running the action as PID 1 buys lifecycle containment. Any process the action orphans is adopted by PID 1 inside the namespace, and when PID 1 exits the kernel `SIGKILL`s every remaining process in the namespace and destroys it. A test that spawns a background daemon, a build step that leaks a subprocess — none of it can survive the action and leak into the next one on the same worker. (During the action a naive PID 1 may leave transient zombies, since a compiler is not written to behave like `init`; they are all reaped when it exits.) This is exactly the "zombie processes" hazard the config docs cite as a reason to enable namespaces (`cas_server.rs:1235-1238`), and the reason the pre-exec hook exists at all (`running_actions_manager.rs:1457-1458`).

**Failure modes.**
- `unshare(CLONE_NEWUSER)` fails with `EPERM` inside an unprivileged container. The worker probes support at startup by forking a throwaway process (`namespaces_supported`, `namespace_utils.rs:81`); if `use_namespaces: true` but the probe fails, the worker **exits** rather than silently degrading (`local_worker.rs:647-653`). Run the worker as a privileged container, grant the right user-namespace kernel permissions, or leave namespaces off.
- With `use_mount_namespace: true`, a failure inside `perform_remount` is a hard `unwrap()` in the pre-exec hook (`namespace_utils.rs:445`): the action fails to spawn rather than running with weaker isolation.
- `gid_map`, `setgroups`, and `sethostname` are best-effort — an `EPERM`/`EACCES`/`ENOENT` on those is swallowed so the action still runs (`namespace_utils.rs:420-458`).

## What Each Level Gives You

| Isolation Level | Filesystem | Process | Network | Use Case |
|----------------|-----------|---------|---------|----------|
| None | Shared host | Shared host | Shared host | Development only |
| Directory only | Separate workdir, read-only inputs | Shared host | Shared host | Basic CI |
| Namespaces (no mount) | Separate workdir, read-only inputs | Isolated PID (action is PID 1) + IPC | Shared host | Standard production |
| Namespaces (with mount) | Siblings masked by tmpfs, private mounts | Isolated PID (action is PID 1) + IPC | Shared host | Strict production |
| Container (via entrypoint) | Container filesystem | Container isolation | Configurable | Maximum isolation |

Two clarifications the table can't fit. **Read-only inputs are not a namespace feature**: they hold at every level from *Directory only* up, because the hardlinked CAS inode is `0o444` (or `0o555` for executables). And **network is `Shared host` for every namespace level** — NativeLink never calls `unshare` with `CLONE_NEWNET`; only a container entrypoint can add network isolation.

## Container-Based Isolation

The strongest isolation comes from running actions inside containers via the entrypoint:

```json5
// Fragment of a `local` worker config (validated as part of a full worker).
local: {
  entrypoint: "/usr/local/bin/container-entrypoint.sh",
  additional_environment: {
    // `property` reads a platform property; `action_directory` is a bare
    // string tag (a unit enum variant), not `{ action_directory: {} }`.
    CONTAINER_IMAGE: { property: "container-image" },
    ACTION_DIRECTORY: "action_directory",
  },
}
```

The entrypoint script launches each action in a fresh container:

```bash
#!/bin/bash
exec docker run --rm \
  --network=none \
  --memory=4g \
  --cpus=4 \
  -v "${ACTION_DIRECTORY}:${ACTION_DIRECTORY}" \
  -w "${ACTION_DIRECTORY}" \
  "${CONTAINER_IMAGE}" "$@"
```

This gives you:
- Full filesystem isolation (container has its own root)
- Network isolation (`--network=none`)
- Resource limits (cgroups via Docker)
- Hermetic toolchain (inside the container image)

The tradeoff is startup latency. Container creation adds 100-500ms per action. For builds with thousands of small actions (e.g., C++ compilation), this overhead is significant. For builds with fewer, longer actions (e.g., integration tests), it's negligible. Reach for a runtime container when you need the OS-level isolation a container boundary provides — untrusted code, a hard network cutoff, or per-action resource caps.

## The Tradeoff Spectrum

```
Faster, less isolated                    Slower, more isolated
├─────────────────────────────────────────────────────────────┤
│  No isolation  │  Namespaces  │  Mount NS  │  Containers   │
│  (dev only)    │  (standard)  │  (strict)  │  (maximum)    │
```

Most production deployments use namespaces without containers. The overhead is a single `fork` plus a handful of `unshare`/`mount` system calls per action — cheap, but emphatically *not* free and *not* "just a kernel flag": every action run in a namespace gets a reaper stub process (see [The reaper stub](#the-reaper-stub-pid-1)). What you buy for that fork is the elimination of the most common isolation failures — leftover processes and daemons (PID 1 tears the whole tree down on exit), shared-memory leaks (IPC namespace), and, with a mount namespace, actions naming each other's files.

Container isolation is used when:
- Actions need specific OS/package combinations (different from the worker host)
- You need strict network isolation (prevent test actions from hitting production services)
- You need resource limits per action (prevent one action from starving others)

## Security Considerations

Namespace isolation is not a security boundary. A determined action can escape user namespaces on older kernels. Container isolation (with properly configured Docker/Podman) is a stronger boundary but still not equivalent to a VM.

For truly untrusted workloads (running arbitrary user code), use:
- Containers with `--security-opt=no-new-privileges`
- Seccomp profiles
- Read-only root filesystem
- Network namespaces with no connectivity
- Consider gVisor or Firecracker for VM-level isolation

For typical build workloads (compiling your own code), namespace isolation is sufficient. The goal is correctness (preventing accidental state leakage), not security (preventing malicious escape).
