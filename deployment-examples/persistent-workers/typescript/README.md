# TypeScript Persistent Worker Example

TypeScript worker wrappers commonly use JSON worker protocol framing. The
important NativeLink requirement is that the remote action advertises both
worker support and the JSON protocol.

```starlark
def _tsc_worker_impl(ctx):
    args = ctx.actions.args()
    args.add("@%s" % ctx.outputs.tsconfig.path)

    ctx.actions.run(
        executable = ctx.executable.tsc_worker,
        arguments = [args],
        inputs = ctx.files.srcs + [ctx.outputs.tsconfig],
        outputs = ctx.outputs.js,
        mnemonic = "TypeScriptCompile",
        execution_requirements = {
            "supports-workers": "1",
            "requires-worker-protocol": "json",
        },
    )
```

The persistent worker should read one newline-delimited JSON `WorkRequest` from
`stdin` and emit one newline-delimited JSON `WorkResponse` to `stdout` for each
action.
