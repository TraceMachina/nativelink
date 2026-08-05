# Persistent Worker Examples

These examples show the NativeLink-facing part of Bazel actions that opt in to
remote persistent worker execution.

Persistent workers are useful for JVM-heavy tools such as `javac`, `scalac`, and
`kotlinc`, and for worker wrappers around tools such as `tsc`. Compatible
actions declare worker support with execution requirements:

```starlark
execution_requirements = {
    "supports-workers": "1",
    "requires-worker-protocol": "proto",  # or "json"
}
```

See:

- [Java](./java/README.md)
- [TypeScript](./typescript/README.md)
- [Kotlin](./kotlin/README.md)
