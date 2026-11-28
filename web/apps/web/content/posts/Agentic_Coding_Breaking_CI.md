---
title: "Agentic Coding Is Breaking Your CI, and Nobody's Talking About It"
tags: ["blog-posts", "ci", "bazel"]
image: /assets/agentic-ci-header.png
slug: agentic-coding-is-breaking-your-ci
pubDate: 2026-11-28
readTime: 8 minutes
excerpt: "Coding agents push dozens of times an hour. That floods queues, craters cache hit rates and erodes hermeticity. Here's what breaks, what to measure, and how to fix it without changing CI providers."
---

**Sumon Sadhu**

<figure>
  <img src="/assets/agentic-ci-header.png" alt="Agentic Coding Is Breaking Your CI: Claude, Codex and Cursor flooding a CI queue into a shattered cache" width="1200" height="627" fetchpriority="high" />
</figure>

Your CI was built for humans. Humans write a change, stare at it, get coffee, and push a handful of times a day. Every assumption in your build platform, from queue sizing to cache eviction to how long a flaky test can hide, was tuned to that rhythm.

Coding agents don't have a rhythm. They have a loop.

## Thirty commits in ten minutes

Picture a scenario that's quickly becoming ordinary. An engineer asks an agent to fix a failing integration test. The agent edits a file, pushes, waits for CI, reads the failure, tweaks a line, and pushes again, thirty times in ten minutes. Meanwhile four teammates are running agents on their own branches.

Nobody did anything wrong. Each commit is small and reasonable. But CI just took in what used to be weeks of one person's pushes in an afternoon, and each one wants a green check before the agent moves on.

## The two clocks drift apart

<figure>
  <img src="/assets/agentic-ci-two-clocks.png" alt="The two clocks: wall time is what you wait for, CPU time is what you pay for" width="1200" height="675" loading="lazy" />
</figure>

Every build has two clocks. Wall time is what the developer, or the agent, waits for. CPU time is what you pay for. Remote caching cuts both by skipping work entirely. Remote execution cuts wall time by running in parallel, but the CPU still gets spent.

Agent traffic stretches both. Wall time rises with the queue. CPU time rises because every miss is real work. Watch only wall time and you'll miss a tripled compute bill. Watch only cost and you'll miss the agents sitting idle.

## Cache hit rates crater

<figure>
  <img src="/assets/agentic-ci-cache-collapse.png" alt="Cache hit rate collapse: a warm cache, then a burst of micro-commits shatters it" width="1200" height="675" loading="lazy" />
</figure>

Bazel doesn't send every action to your executors. It asks the action cache first (GetActionResult) and only calls Execute on a miss [1][2]. Load on your workers therefore scales with cache misses, not pushes.

Agent traffic raises the number of pushes _and_ lowers the hit rate. Thirty near-identical variants of the same change each produce slightly different action keys, which fill the cache with entries nobody will reuse and push out the ones people would. Touch a shared header or a lockfile and a wide slice of the graph misses at once. Teams that publish numbers report hit rates in the 70 to 90% range on mature, hermetic setups [5][6], and a former Google Bazel engineer argues caching only really fixes build times above about 90% [7]. Agent churn pulls you down from there.

## Queue depth goes vertical

<figure>
  <img src="/assets/agentic-ci-queue-depth.png" alt="Queue depth spike: a scheduler flooded with actions, most of them already out of date" width="1200" height="675" loading="lazy" />
</figure>

Agents block on the result. A human pushes and switches tasks. An agent sits idle until CI answers, so every minute of queue wait goes straight into the loop. Arrivals outpace completions, and every new job waits behind every old one.

Worse, many of those runs are obsolete before they start. Commit 12 is superseded by commit 13 while it's still queued, but unless something cancels it, it still takes a worker.

## Hermeticity quietly degrades

<figure>
  <img src="/assets/agentic-ci-hermeticity.png" alt="Hermeticity degrades: a clean sandbox cracked by inputs nobody declared" width="1200" height="675" loading="lazy" />
</figure>

Caching only works if an action's key really describes its inputs. Agents are great at making things pass and bad at noticing _why_ they pass. A test that reads `$HOME`, a script that calls whatever `python3` is on the path, or a fetch from the network at build time all work fine locally. Each one is an undeclared input. The key stays the same while the result changes, so you get cache misses at best and wrong cached results at worst. One team rolling out remote execution found most of its misses came from exactly this: toolchain paths, locale, timestamps and test environment variables [6].

## Presubmit is now a product

<figure>
  <img src="/assets/agentic-ci-presubmit-postsubmit.png" alt="Presubmit vs postsubmit: one gate before merge, one ledger after it" width="1200" height="675" loading="lazy" />
</figure>

Presubmit is the gate a change must pass before merge. Postsubmit is the record you check after. When humans were the only authors, a slow presubmit was an annoyance. When agents are authors, presubmit is the interface they work through, and its speed and trustworthiness set how fast they can go. Keep presubmit scoped and fast, push the broad, slow suites to postsubmit where you can bisect, and treat both as products with owners and service levels.

## What NativeLink does about it

NativeLink is an open source remote cache and remote execution server that speaks the same Remote Execution API as Bazel [2]. It runs on your own hardware. Any client that speaks that API can use its cache and execution. Buck2 does, for example [10], though Bazel is by far the most common and best-documented path, and it's the one this post uses.

Here's a trimmed server config. It shows a filesystem CAS and action cache, each wrapped in `cache_metrics` so hit rates get reported, a scheduler that matches on CPU count, and a mutual-TLS listener. Check the reference configs in the repo for the full shape.

```json5
{
  stores: [
    {
      name: "CAS",
      cache_metrics: {
        cache_type: "cas",
        backend: {
          filesystem: {
            content_path: "/var/nativelink/cas",
            temp_path: "/var/nativelink/tmp",
            eviction_policy: { max_bytes: 500000000000 },
          },
        },
      },
    },
    {
      name: "AC",
      cache_metrics: {
        cache_type: "ac",
        backend: {
          filesystem: {
            content_path: "/var/nativelink/ac",
            temp_path: "/var/nativelink/tmp",
            eviction_policy: { max_bytes: 10000000000 },
          },
        },
      },
    },
  ],
  schedulers: [
    {
      name: "MAIN",
      simple: {
        supported_platform_properties: {
          cpu_count: "minimum",
          OSFamily: "exact",
        },
      },
    },
  ],
  servers: [
    {
      name: "public",
      listener: {
        http: {
          socket_address: "0.0.0.0:50051",
          tls: {
            cert_file: "/etc/nativelink/server.crt",
            key_file: "/etc/nativelink/server.key",
            client_ca_file: "/etc/nativelink/clients-ca.crt",
          },
        },
      },
      services: {
        cas: [{ instance_name: "", cas_store: "CAS" }],
        ac: [{ instance_name: "", ac_store: "AC" }],
        execution: [{ instance_name: "", cas_store: "CAS", scheduler: "MAIN" }],
        capabilities: [
          { instance_name: "", remote_execution: { scheduler: "MAIN" } },
        ],
      },
    },
  ],
}
```

Workers on other machines need to reach the same CAS, so point their stores at the server's CAS over gRPC instead of a local disk. Authentication is mutual TLS: clients present a certificate signed by `client_ca_file`. NativeLink has no token auth.

On the Bazel side:

```
build --remote_executor=grpcs://nativelink.internal:50051
build --tls_certificate=/etc/bazel/ca.crt
build --tls_client_certificate=/etc/bazel/client.crt
build --tls_client_key=/etc/bazel/client.key
```

`--remote_executor` implies the remote cache, so you don't also need `--remote_cache`. One gotcha: `--remote_default_exec_properties` is ignored for any platform that sets its own `exec_properties`, so put `cpu_count` and `OSFamily` on the platform.

Watch four metrics, exported over OTLP (set `OTEL_EXPORTER_OTLP_ENDPOINT`) [9]:

- `cache.operations`, for hit rate. It's only emitted for stores wrapped in `cache_metrics`.
- `scheduler.queue.depth`, for how far arrivals are outrunning workers.
- `execution.queue.time`, for how long actions wait before they run.
- `worker.state.count`, broken down by `worker.state`, for idle versus busy capacity.

Also watch `scheduler.unsatisfiable.queued`. If it climbs, actions are asking for properties no worker has.

### Getting started

Run the quickstart from the NativeLink README, which starts the published container with a basic CAS config, then point one Bazel target at it with `--remote_cache` and confirm hits on a second build. Add remote execution once caching works.

## Keep your CI, add the cache

You don't have to leave GitHub Actions or Buildkite to get any of this. NativeLink isn't a CI system. It's a remote cache and execution endpoint, and Bazel finds it through flags, not through your CI provider. A GitHub-hosted runner, a Buildkite agent and a laptop can all point at the same endpoint with the same `--remote_executor` or `--remote_cache` flags, and share the same cache.

Put the flags in a `.bazelrc` config so every CI job uses the same settings:

```
# .bazelrc
build:nativelink --remote_executor=grpcs://nativelink.internal:50051
build:nativelink --tls_certificate=ca.crt
build:nativelink --tls_client_certificate=client.crt
build:nativelink --tls_client_key=client.key
```

A Buildkite step, with the client certificate already on the agent:

```yaml
steps:
  - label: ":bazel: test"
    command: bazel test //... --config=nativelink
```

A GitHub Actions job, writing the certificates from repository secrets first:

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: |
          echo "${{ secrets.NL_CA }}"   > ca.crt
          echo "${{ secrets.NL_CERT }}" > client.crt
          echo "${{ secrets.NL_KEY }}"  > client.key
      - run: bazel test //... --config=nativelink
```

A hosted runner has to be able to reach the endpoint, so either expose it over the internet behind mutual TLS, or use self-hosted runners or agents inside your network.

That makes adoption incremental. Start with `--remote_cache` on one pipeline and watch `cache.operations`. Turn on remote execution for the targets that benefit, then move more pipelines over. Your CI provider, its UI and its triggers stay exactly as they are. Only where the work runs, and how often it runs at all, changes.

## Why run it on your own servers

Two separate savings are at work here, and it's worth keeping them apart.

**Saving one: the cache cuts minutes wherever CI runs.** A cache hit replaces an action's full CPU time with a lookup and a download. That works the same whether your runners are GitHub's or your own, so you can point hosted runners at a remote cache and bill fewer minutes the same day. Tuist's public dashboard for one Bazel project shows 88.9% of action cache lookups served from cache over 30 days [5]. An independent team reported a 71% average after fixing non-hermetic inputs [6]. Spotify cut CI feedback from 80 to 20 minutes with a remote cache, remote execution and affected-target selection [8]. Treat 70 to 90% as what mature, hermetic setups report, not a starting point.

**Saving two: owning the hardware changes the price of the minutes left.** GitHub Actions charges $0.006 a minute for a 2-core Linux runner and $0.022 for 8 cores, rounded up per job [3]. At 1,000 uncached 8-core minutes a day, that's about $660 a month, and it grows in a straight line with agent volume. Some providers, like Buildkite, charge per user instead, so compare total monthly cost, not per-minute rates [4]. On owned machines, the monthly cost is hardware divided by its useful life, plus power, plus rack or data center space, plus the share of an engineer's time spent running it. Self-hosting tends to win when the leftover minutes are high and steady, and to lose when they're small or spiky.

**Data locality and control.** Source and artifacts never go to a third party. You also set queue policy, worker sizes and retention, so agent traffic gets its own lane.

## The CI loop is now the product

Build performance used to be a quality-of-life problem. Slow CI annoyed engineers but didn't stop them. Agents change that, because their speed is capped by your feedback loop. A ten-minute presubmit was fine for someone who pushes five times a day. For an agent that needs thirty iterations, it can mean five hours of waiting.

The teams that win with coding agents won't be the ones with the cleverest prompts. They'll be the ones whose build systems give fast, trustworthy answers again and again at machine speed: hermetic builds, high hit rates, short queues, and telemetry that can tell an agent from a human.

Your agents are only as fast as your CI. Build CI for the agents.

## References

1. [Bazel remote caching documentation](https://bazel.build/remote/caching)
2. [Remote Execution API (REAPI v2)](https://github.com/bazelbuild/remote-apis)
3. [GitHub Actions runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing), checked November 28, 2026
4. [Buildkite pricing](https://buildkite.com/pricing), checked November 28, 2026
5. [Tuist, "Bazel remote caching and insights in Tuist"](https://tuist.dev/blog/2026/09/09/bazel)
6. [Fred's Dev Blog, Bazel remote execution case study](https://fred-dev-blog.ellijobs.com/posts/bazel-rbe-action-boundary/) (Korean)
7. [Julio Merino, "How does Google keep build times low?"](https://jmmv.dev/2021/02/google-monorepos-and-caching.html)
8. [Spotify Engineering, "Switching Build Systems, Seamlessly"](https://engineering.atspotify.com/2023/10/switching-build-systems-seamlessly)
9. [NativeLink metrics README](https://github.com/TraceMachina/nativelink/tree/main/deployment-examples/metrics)
10. [Buck2 remote execution documentation](https://buck2.build/docs/users/remote_execution/)
