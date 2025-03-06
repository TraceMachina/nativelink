---
title: "Thirdwave Automation and NativeLink"
tags: ["news", "case-studies"]
image: https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_logo.webp
slug: case-study-thirdwave-automation
pubDate: 2025-03-06
readTime: 7 minutes
---


####

#### *How NativeLink’s open software development and validation platform helped a robotics innovator deliver safer products faster (at less cost)*

“Our business relies on delivering autonomous systems with an exceptional level of safety. With NativeLink’s massively-parallel cloud service, we can now iterate faster and continuously learn from our testing, without wasting time managing build cycles and cloud resources. It’s very fast, stable. It just works.”

Nate Gallaher, DevOps Team Lead, Thirdwave Automation

### Summary

Founded in 2018 and funded by Toyota’s growth fund plus Innovation Endeavors, Norwest Venture Partners, Woven Capital, and Qualcomm Ventures, Third Wave Automation (TWA) leverages machine learning and artificial intelligence to manage fleets of automated forklifts and respond to edge cases in a timely and effective manner.

Using automotive grade 3D LIDAR, TWA’s robotic forklifts incorporate Collision Shield, the industry-leading autonomous obstacle detection system running on forklifts. Other innovations include its Shared Autonomy Platform, enabling the TWA Reach line of forklifts to operate autonomously or seek help from remote operators who can take control from the safety of their office.

Key elements of the company’s value proposition are safety, scalability, adaptability, repeatability, stability, and affordability. In order to preserve developer velocity and ensure timely delivery of its solutions, Thirdwave turned to NativeLink to help take its build and validation infrastructure to the next level.

### Key Outcomes

* 80% reduction in build times
* 50% reduction in cloud costs
* Transparent cloud scaling (Running 50K simultaneous jobs)
* Now testing daily vs. weekly or monthly
* Hundreds of developer hours saved per year
* Rapid deployment | Bazel compatible

### The Challenge

The company develops complex software for intelligent material-handing, including semi-autonomous robots as well as a fleet management system with remote operation and assistance capabilities.

Written in C/C++ and Python, the TWA software is built upon a micro-services architecture that makes extensive use of containers.

As the code base and number of developers have grown, the team's ability to iterate quickly was significantly hindered by prolonged build and test times, reducing developer productivity.

But given the critical importance of safety, it became even more important to run testing on a more frequent basis, including unit tests, subsystem tests, and system-wide tests. Keeping developer velocity high by reducing test turnaround time was also a key requirement.

At the same time, the company was concerned it was running compilation tasks on the same expensive GPUs as their ML tests. They were looking for a more flexible and configurable approach to save on cloud computing costs.

### The Solution

Massively parallel architecture with remote execution and caching

Leveraging NativeLink’s remote execution and build caching platform, built on a massively-parallel, cloud-optimized service, the firm reduced build times by up to 80% while also removing developer overhead and friction.

Incremental builds are also faster because cache artifacts are reused whenever possible. Developers never need to build or test the same thing twice and can share code across different environments.

Nate reports that having faster cycles actually changed developer behavior by encouraging developers to run tests more frequently, without concerns about potential bottlenecks.

Deterministic builds

As a deterministic build system, NativeLink prevents cross-platform divergence by enforcing a hermetic toolchain and pinning all external dependencies to a given version, helping to prevent the common occurrence of “but it worked fine on my machine” syndrome.

This also helps enforce critical governance controls around the software supply chain, preventing tampering and producing an SBOM required by compliance mandates.

Open and extensible platform built on Bazel

Licensed as Apache 2.0 open source, NativeLink seamlessly integrates with Bazel, the modern build client developed by Google and now used by 1,000+ organizations including Tesla, Nvidia, Ford, Stripe, Dropbox, Datadog, and Databricks.

NativeLink works with all client-side build tools that support the Remote Bazel Execution (RBE) protocol such as Bazel, Buck2, Pantsbuild, and Reclient.

You can customize NativeLink via JSON or YAML, as well as via Starlark, a Python-based declarative language for configuring Bazel with custom build rules and macros for specific projects and platforms.

### About NativeLink

NativeLink is the world's first open software development and validation platform purpose-built for diverse target platforms such as robots, autonomous vehicles, and edge devices.

The platform is also optimized for developers building other types of large artifacts such as Chromium-based browsers, system software, and next-generation semiconductors.

Designed to meet the growing scalability demands and complexity of large-scale software projects, NativeLink accelerates time-to-market by 10x or more without sacrificing reliability, safety, security, or efficiency.

NativeLink is currently deployed in some of the world's largest and most complex environments, including Brex, Citrix, and Menlo Security.

TraceMachina, developer of NativeLink, is backed by leading investors including Sequoia, Wellington Management, Verissimo Ventures, and prominent angel investors.

To learn more about NativeLink, read our documentation, check out our GitHub Repository, or contact our team directly at [hello@nativelink.com](mailto:hello@nativelink.com).
