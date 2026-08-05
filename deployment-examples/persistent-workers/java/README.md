# Java Persistent Worker Example

This example shows the NativeLink-facing part of a Java compile action that can
run through the remote persistent worker path. The action must declare that the
tool supports Bazel's worker protocol and that it uses the proto wire format.

```starlark
def _javac_worker_impl(ctx):
    args = ctx.actions.args()
    args.add("@%s" % ctx.outputs.argfile.path)

    ctx.actions.run(
        executable = ctx.executable.javac_worker,
        arguments = [args],
        inputs = ctx.files.srcs + [ctx.outputs.argfile],
        outputs = [ctx.outputs.jar],
        mnemonic = "Javac",
        execution_requirements = {
            "supports-workers": "1",
            "requires-worker-protocol": "proto",
        },
    )
```

NativeLink computes the `WorkerKey` from the executable, startup flags before the
first `@argfile`, and `requires-worker-protocol`. The first action starts
`javac_worker --persistent_worker`; later compatible actions send `WorkRequest`
messages to the same process.
