# Summary

[Introduction](./introduction.md)

---

# Part I: For Operators

# What This Is

- [The 30-Second Model](./part1/thirty-seconds.md)
- [NativeLink vs. Everything Else](./part1/vs-everything.md)
- [Mental Model](./part1/mental-model.md)
- [The CAS Is the Spine](./part1/the-cas-is-the-spine.md)

---

# The Protocol

- [REAPI from First Principles](./part2/reapi.md)
- [The Execution Contract](./part2/execution-contract.md)
- [Why Content-Addressing Works](./part2/content-addressing.md)
- [Where REAPI Breaks Down](./part2/reapi-breaks.md)

---

# Stores

- [The Store Trait](./part3/store-trait.md)
- [Store Composition](./part3/store-composition.md)
- [Cloud Backends](./part3/cloud-backends.md)
- [The Store Catalog](./part3/store-catalog.md)

---

# Scheduling and Execution

- [The Scheduler](./part4/scheduler.md)
- [Platform Properties](./part4/platform-properties.md)
- [Workers](./part4/workers.md)
- [Sandboxing and Isolation](./part4/sandboxing.md)

---

# Toolchains

- [The Toolchain Problem](./part5/toolchain-problem.md)
- [Nix and LRE](./part5/nix-lre.md)
- [Container-Based Toolchains](./part5/containers.md)
- [Hermetic Toolchains Without Nix](./part5/hermetic-without-nix.md)
- [Platform Properties as the Bridge](./part5/platform-bridge.md)

---

# Client Integration

- [Bazel](./part6/bazel.md)
- [Buck2](./part6/buck2.md)
- [Other Clients](./part6/other-clients.md)
- [Debugging Cache Misses](./part6/debugging-cache-misses.md)

---

# Deployment

- [Single Node](./part7/single-node.md)
- [Multi-Worker](./part7/multi-worker.md)
- [Kubernetes Production](./part7/kubernetes.md)
- [Observability](./part7/observability.md)

---

# Worked Examples

- [From Scratch: Bazel + NativeLink](./part8/bazel-from-scratch.md)
- [From Scratch: Buck2 + NativeLink](./part8/buck2-from-scratch.md)
- [Local Remote Execution with Nix](./part8/lre-nix.md)
- [Multi-Team Production Config](./part8/multi-team.md)

---

# Part II: For Contributors

- [Part II: For Contributors](./contributors/00-contributors-intro.md)
- [Orientation: The Shape of the Codebase](./contributors/01-orientation.md)
- [Building and Testing](./contributors/02-building-and-testing.md)
- [The Core Abstractions](./contributors/03-core-abstractions.md)
- [The Data Path, End to End](./contributors/04-data-path.md)
- [Recipes](./contributors/05-recipes.md)
- [Concurrency and Correctness](./contributors/06-concurrency-and-correctness.md)
- [Contributing: Getting a Change Merged](./contributors/07-contributing.md)

---

# Appendices

- [A: Configuration Reference](./appendix/config-reference.md)
- [B: Scheduler Catalog](./appendix/scheduler-catalog.md)
- [C: Troubleshooting](./appendix/troubleshooting.md)
- [D: Glossary](./appendix/glossary.md)
