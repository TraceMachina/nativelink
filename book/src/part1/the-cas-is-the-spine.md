# The CAS Is the Spine

*What cancels once you factor it out.*

## The factor

Every build system, every registry, every cache, every artifact store is doing the same thing, badly, N times. They store bytes. They name the bytes. They garbage-collect the bytes, replicate the bytes, check the bytes haven't rotted, refuse to store the same bytes twice, decide who may read the bytes. Git does it. OCI registries do it. Nix's binary cache does it. Bazel's remote cache does it. Your CDN does it. Your SBOM pipeline does a degenerate, drifting version of it.

Write the whole edifice as a fraction — modern software infrastructure over its irreducible content — and the same factor sits in every term: a content-addressed store. Not "a store that happens to hash things." A store where **the name *is* the content**, and **the content carries its own lineage**. Pull that factor out front, and start crossing terms.

Once the factor is visible the cancellation is violent. Subsystems that looked load-bearing turn out to be the same thing written twice, and they annihilate.

## What cancels

**Storage engines cancel.** You don't build git storage, OCI storage, nix storage, and REAPI storage. You build one store. "The OCI registry" stops being a system and becomes a naming convention — a few hundred lines on how to lay out a manifest and address a layer. Nix's cache is a narinfo veneer; git is a packfile veneer; REAPI is a Directory-proto veneer. Each protocol is a **lens**: a way of looking at the one store through a chosen wire format. N products collapse to one store and N thin lenses — and a lens has no storage, no GC, no replication. It has a direction to look.

**Dedup cancels.** It was never a feature. If the name is the content, you *cannot* store the same thing twice; the address function already did it. Every "deduplicating store" is a store that hasn't yet admitted its addresses are content. Cross the whole column out.

**Garbage collection cancels — down to one.** Reachability over a Merkle DAG. The same collector that reclaims a stale build output reclaims a dangling git blob reclaims an orphaned toolchain layer, because after the factoring they are the same kind of node. You had N collectors, each subtly wrong. You have one, and it's the only one that was ever needed.

**Integrity checking cancels.** Corruption is not something you scan for; it's something that cannot hide. A blob whose bytes don't hash to its name isn't "possibly corrupt pending a checksum" — it is *provably* not the thing it claims to be. Detection is the identity function. Self-healing is evict-and-refetch, and it falls out for free, not from a cron job.

**Caching cancels into the substrate.** No separate "cache" exists. A cache is the store with an eviction policy. The action cache, the local NVMe fast tier, the R2 cold tier, the CDN edge — one object at different eviction grades. And the famous hard problem, cache invalidation, largely *evaporates*: you never invalidate content, because content is immutable and self-naming; there's nothing to invalidate. What remains is a small, honest, separable concern — mutable **name → content** bindings — which you invalidate explicitly and cheaply, instead of invalidation logic smeared across every subsystem as ambient dread.

**"Local vs remote" cancels.** If identity is content — not the recipe, not the path, not the machine — then a thing built locally and the same thing built on a worker share a *name*, hence a cache key, hence are not two things to reconcile. They're one thing computed in two places. "Make remote match local" was never a task; it was a symptom of addressing by location. Local is just another worker. The entire genre of environment-drift bug crosses out.

**Toolchains-as-infrastructure cancels.** A worker stops being a machine you provision, image, and keep in sync. The toolchain is content; the worker fetches it like any other input and discharges it. Redeploying workers to bump a compiler, per-worker container images, "which clang is on node 7" — gone. The worker becomes stateless: a discharger with a floor loader and a hash function.

**Input-addressing cancels — and takes its trust economy with it.** Address by recipe (a derivation hash, a Dockerfile digest) and you're trusting that the recipe reproduces; you've bought a supply chain of *hope*. Address by content and you don't trust, you *check*: the artifact is or is not the expected hash, one comparison. Two different recipes producing identical output now dedup, which input-addressing structurally cannot do. And the store path stops being identity — it demotes to a materialization detail. Path-rewriting, closure surgery, "the path is the truth" tax: crossed out. (A content-resolving loader is this cancellation made physical — it binds dependencies by digest, not by location. Location ≠ identity. The paths leave the numerator entirely.)

**The SBOM cancels — because it was already there.** You don't generate a bill of materials, store it beside the artifact, and pray they agree. The Merkle DAG *is* the bill of materials — exactly and always, and it cannot drift from the artifact because it *is* the artifact's shape. "What went into this?" is a graph walk, not a document. The class of "the attestation says X, the artifact contains Y" isn't a bug you fix; it's a state that cannot be represented. Attestation in the genome, not stapled to the skin.

**The correctness questions unify into one.** Once a cache hit means *the coeffect is discharged* rather than *the bytes match*, a pile of mechanisms — hermeticity checks, toolchain pinning, reproducibility audits, "did the environment change" — collapse into a single question: **is the graded demand met?** N checks over N subsystems become one check over a grade. And a hint is a **discharge receipt**: "already proved, carry the certificate, don't recompute." Handoff between tiers, nodes, and lenses stops re-deriving at every boundary — precisely the tax that shows up as multi-thousand-blob re-uploads and read-fan-out timeouts: a boundary that recomputed instead of carrying a receipt.

## The residue

Cross everything out and look at what refuses to cancel. That remainder is the only thing you're obligated to build — and to build without compromise, because there's no second thing to hide behind:

- **the store** — name = content, arbitrarily shardable (hash → node), append-only, verify-on-read;
- **the genome** — lineage and attestation inseparable from content, so provenance is a walk and corruption cannot hide;
- **the grades** — coeffects first-class, so *valid* means *demand discharged*, and receipts make handoff free;
- **placement and eviction** — self-healing replication and tiering, because the store is the cache is the CDN is the cold tier.

That is the whole obligation. Small, and brutal — *everything* rests on it. But get it right and you're not left with a build system that also caches. You're left with one substrate and a drawer of lenses, and the next facade — a git endpoint, a container registry, a package index, a thing no one has named yet — costs a wire format and an afternoon, because the hard part was factored out and cancelled long ago.

## The inversion

You don't arrive here by adding features to a build system until it becomes a platform. That road writes its own confession into the config: stores, schedulers, and services as *peers* in an array, threaded by reference — an architecture where the CAS is one component among many and every new capability is a new coupling. That's optimizing to plug into a zillion ecosystems before you've learned the shape: reasonable for a product, fatal for a foundation.

You arrive by inversion. Make the CAS so complete — so scalable, self-healing, attested, and graded — that it stops being the storage layer *under* the system and becomes the system, and everything that was a subsystem demotes to a view. Git, the registry, the nix cache, the REAPI cache, the SBOM, the audit log: `SELECT`s over one table. God's own hash table — and the whole industry is a query planner that forgot the table was the point.
