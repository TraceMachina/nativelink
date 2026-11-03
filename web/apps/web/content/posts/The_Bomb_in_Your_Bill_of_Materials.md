---
title: "The Bomb in Your Bill of Materials"
tags: ["security", "blog-posts"]
image: /assets/sbom-hidden-risk.png
slug: the-bomb-in-your-bill-of-materials
pubDate: 2026-11-03
readTime: 4 minutes
---

**Marcus Eagan**

<figure>
  <img src="/assets/sbom-hidden-risk.png" alt="An amber dependency with a glowing fuse is revealed inside a stack of connected software components and highlighted in a bill of materials." width="1536" height="1024" fetchpriority="high" />
  <figcaption>A bill of materials makes hidden dependencies visible. Each flagged component still needs an answer.</figcaption>
</figure>

> "You dropped a bomb on me, baby<br />
> You dropped a bomb on me (I won't forgive what you done)<br />
> You dropped a bomb on me, baby<br />
> You dropped a bomb on me<br />
> I, I-I, I-I-I won't forget it<br />
> I, I-I, I-I-I won't forget it"
>
> — [The Gap Band, "You Dropped a Bomb on Me"](https://www.youtube.com/watch?v=17lkdqoLt44)

Most software bombs don't go off the day they're dropped. They sit in a dependency tree, flagged and filed as low priority, until someone with patience and a motive comes looking. With coding agents, that someone shows up faster, and there are more of them.

## An SBOM is a living record

In NativeLink, the software bill of materials comes out of how we build the project. Our environments are managed with Nix, with declared and pinned dependencies that make builds reproducible. When a scanner flags a component, we treat the flag as a question we owe an answer to, not noise to suppress.

The industry needs to adopt open standards like SPDX, CycloneDX and OpenVEX faster, alongside tools like [OpenSSF Scorecard](https://scorecard.dev/). A bill of materials only helps if the people downstream can read it, merge it and act on it with the same tools.

<figure>
  <img src="/assets/sbom-dependency-tree.png" alt="A magnifying glass reveals an amber component several levels down a branching software dependency tree." width="1536" height="1024" loading="lazy" decoding="async" />
  <figcaption>A vulnerable component can sit several layers below the application that depends on it.</figcaption>
</figure>

## The fuse is long

The zstd ecosystem shows why. In August, zstd-jni disclosed [out-of-bounds read](https://github.com/luben/zstd-jni/security/advisories/GHSA-ff36-7w3w-g8rm) and [use-after-free](https://github.com/luben/zstd-jni/security/advisories/GHSA-947w-pxjj-c7m9) bugs that can crash the JVM or corrupt data. On October 12, an advisory for the Go zstd decoder in klauspost/compress [described a panic on crafted two-block frames](https://github.com/klauspost/compress/security/advisories/GHSA-fpmh-frq8-7rj8).

On a quick read, these are crash bugs in someone else's language binding. Not our problem. But compression sits on the hot path of many cache and remote execution systems. A panic is a denial of service today. A memory-safety bug left unpatched for two years can become a building block once someone, human or agent, decides to chain it. A flag that looks harmless still needs a deadline.

For our own zstd dependency fixes in [NativeLink v1.8.3](https://github.com/TraceMachina/nativelink/releases/tag/v1.8.3), it took us thirty minutes total to patch the issues and validate against our NativeLink-powered verification harness, not including nightly harnesses we employ for more extensive soak tests and esoteric fault injections.

## Fast fixes, informed by the build

NativeLink lets us patch quickly because we can check a fix for regressions. When we bump a compression library, remote execution reruns the tests and benchmarks across platforms in minutes, so the security fix and the performance check ship together. Once patching is cheap, teams stop bargaining with their vulnerability backlog.

<figure>
  <img src="/assets/sbom-patch-verification.png" alt="An amber component is replaced with a violet patch, then checked across four parallel verification stations before the paths converge into a release package." width="1536" height="1024" loading="lazy" decoding="async" />
  <figcaption>Patch the dependency, verify the change across workers, and carry the results into the release.</figcaption>
</figure>

It also shapes how we think about security. When every action declares its inputs, an unexpected input stands out. We work to keep our OpenSSF Scorecard as high as we can.

## Nobody is bulletproof

None of this makes us bulletproof. Everyone is vulnerable to attack, us included. We are all just doing our best. Openness and OpenSSF are not a panacea either.

We see a growing risk from two directions: coding agents running on abliterated models with their safety training stripped out, and teams that hand all of their security thinking to an agent. Threat models still need to include humans and nation states, patient adversaries who read the same CVE feeds we do.

We are grateful to the frontier labs, including Anthropic, OpenAI and Google DeepMind, whose models help maintainers like us find and fix bugs faster. Keep humans in the loop, and treat every quiet flag as a lit fuse.

“You dropped a [S]BOMb on Me, agent, you dropped a bomb on me”

## Sources

- [GHSA-ff36-7w3w-g8rm: zstd-jni ZstdDictCompress out-of-bounds read](https://github.com/luben/zstd-jni/security/advisories/GHSA-ff36-7w3w-g8rm)
- [CVE-2026-87825: zstd-jni use-after-free (SentinelOne)](https://www.sentinelone.com/vulnerability-database/cve-2026-87825/) and the [upstream advisory](https://github.com/luben/zstd-jni/security/advisories/GHSA-947w-pxjj-c7m9)
- [CVE-2026-116591: klauspost/compress zstd panic on two-block frames](https://github.com/klauspost/compress/security/advisories/GHSA-fpmh-frq8-7rj8)
- [NativeLink v1.8.3 release notes](https://github.com/TraceMachina/nativelink/releases/tag/v1.8.3)
