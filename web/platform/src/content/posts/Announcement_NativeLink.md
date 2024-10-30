---
title: "Nativelink - The free & open source simulation infrastructure platform written in Rust"
tags: ["news", "announcements"]
image: https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_open_source.webp
slug: NavtiveLink
readTime: 30 seconds
pubDate: 2024-09-17
---
Today, we're excited to release NativeLink–a Rust implementation of Bazel's
Remote Build Execution protocol (RBE) and Content Addressable Storage (CAS)
designed to run your code and get out of the way. NativeLink seamlessly
integrates with Bazel, Reclient, Goma, and Buck2. It comes at no cost and
works on every major operating system.


Offering a vendor neutral solution out the gate was important to us and we
will soon offer more options. Part of vendor neutrality began with
supporting four different build systems that implement the RBE protocol.
Given our focus on mission-critical systems with native code,
reproducibility and hermiticity have been our guiding lights.


Based on feedback from our earliest users, we made a few design choices
around the runtime, I/O, and software supply chain. We don't have any
garbage collection, supporting more deterministic and predictable
execution. In addition, the tunable and highly performant asynchronicity
afforded by Rust has allowed us to achieve concurrency and memory safety that were previously impracticable in most industries prior to its
creation. Ultimately, we  strive for scale, modern SBOM transparency, and
verifiability. The software supply chain is the biggest threat to computer
security in 2023. For mission critical products, the risks are much
greater. Critical systems can't rely on closed source.

Your contributions, questions, and community support are encouraged. Join
our Slack to learn more.

The free & open source simulation infrastructure platform, in Rust
