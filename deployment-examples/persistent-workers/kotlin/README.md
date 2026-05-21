# Kotlin Persistent Worker Example

Kotlin compile actions have the same remote execution shape as Java: keep the JVM
compiler process warm and send per-action argument files through the worker protocol.

```starlark
def _kotlinc_worker_impl(ctx):
    args = ctx.actions.args()
    args.add("@%s" % ctx.outputs.argfile.path)

    ctx.actions.run(
        executable = ctx.executable.kotlinc_worker,
        arguments = [args],
        inputs = ctx.files.srcs + [ctx.outputs.argfile],
        outputs = [ctx.outputs.jar],
        mnemonic = "KotlinCompile",
        execution_requirements = {
            "supports-workers": "1",
            "requires-worker-protocol": "proto",
        },
    )
```

The worker process is started once with `--persistent_worker`; compatible Kotlin
compile actions reuse it until the pool idle timeout or request cap retires it.
