---
title: "Overcoming Common LLVM Build Challenges with NativeLink"
tags: ["blog"]
image:  https://nativelink-cdn.s3.us-east-1.amazonaws.com/LLVMWyvernSmall.webp
slug: building-llvm
pubDate: 2024-10-31
readTime: 4 minutes
---
<!-- vale Style.Rule = NO -->
Formerly known as Low-level virtual machine, LLVM originally started as a new form of virtual machine. Yet, it has evolved to include a broader suite of compiler tools and libraries for building compilers, optimizers, and other code-generation tools. Since 2011, the acronym was dropped. Now, LLVM represents the larger brand, including the LLVM intermediate representation (IR) and the LLVM debugger. The LLVM foundation plays a critical role in the strategic direction of the LLVM project and in fostering an active community to help drive contributors and project adoption.

LLVM is a cornerstone of compiler development now. Its modularity, optimization capabilities, and extensive support for various architectures impact numerous projects. Many compilers leverage LLVM’s capabilities to produce machine code, including Clang, Rust, Swift and Kotlin. For example in the Rust compiler, rustc, LLVM provides the LLVM IR (Intermediate Representation), a very low-level programming language designed to be targeted by compilers. From there, rustc takes your source code, converts it to LLVM IR, and then LLVM handles the conversion of this IR to different CPU architectures, such as x86, ARM, and RISC-V. This makes Rust highly portable and suitable for various platforms.

While developers might not interact directly with LLVM, it has a foundational role in modern compiler development. For example, AMD's ROCm stack for GPU programming is built on LLVM. Similarly, various GPU pipelines use LLVM. So even if you're using higher-level languages like Rust or C++, the performance and capabilities of those languages are directly influenced by LLVM.

Improving LLVM can lead to significant performance enhancements. For example, MLIR (Multi-Level Intermediate Representation), which is part of the LLVM project, allows for more structured and efficient optimizations. Many projects like [Mojo](https://docs.modular.com/mojo/faq#how-does-mojo-support-hardware-lowering) and machine learning frameworks such as TensorFlow and PyTorch use MLIR for their compilers. For example, Mojo leverages MLIR-based code-generation backends and creates machine code that is more optimized for machine learning and artificial intelligence applications when compared to the backends of other languages like Rust and C++.

As you can see, many projects and programming languages continually evolve and integrate with LLVM to take advantage of its advanced capabilities. It offers a modular and reusable framework that simplifies the creation and maintenance of language front-ends and optimizers. Ensuring that the projects that depend on LLVM can continually build it without bottlenecks or failures is vital. Next let’s go over some of the challenges projects face with building LLVM.

Challenges with Build Processes in LLVM
As mentioned earlier, LLVM is one of the largest dependencies that compilers and toolchains use. Achieving efficient and consistent builds is how projects become reliable and performant. Yet, developers not only need to constantly overcome the challenges of long build times but also face environmental and reproducibility inconsistencies that can take hours or days to debug. Additionally, other issues that complicate the build process are high resource utilization, cluster scalability, and performance overheads from containerization. Below are a few examples of how builds can be challenging:

Local Builds
Local builds are constrained by limited CPU and memory resources built into the developer’s machine. Additionally, the developer’s local machine has a single architecture, like x86_64. Testing and validating other architectures, like AMDGPU, is challenging because the local machine’s hardware does not natively support them.

Reproducing local builds across different machines
Reproducing local builds consistently across different machines is not an easy thing to accomplish. It's pretty straightforward to make changes on a local machine: you run and compile the code, and immediately see the impact. However, the process becomes more complex and resource-intensive when you want to do this in a Kubernetes cluster. A developer must build a new Docker image, delete the old pod, replace it with the new one, and finally run the updated code in the cluster. The Kubernetes workflow is significantly slower and more expensive compared to local changes.

Now, achieving equivalence between local and cloud environments is important for development, but also hard to do. When local systems run different operating systems compared to cloud environments, developers will get inconsistent build results because these systems can't share caches. Sharing caches ensures all previously built artifacts are reused, thereby saving time and compute resources. Without it, each build must start from scratch increasing the build time and compute resources needed. Local Remote Execution (LRE) bridges this gap ensuring both environments share the same cache and dependencies even if they have different operating systems. LRE not only reduces the overhead by reducing the build times and CPU cycles needed, but it also improves developer productivity.

## Kubernetes
Many engineers struggle with scalability and resource allocation in Kubernetes clusters. Multiple jobs may be scheduled on the same node, competing for CPU and memory resources. If the resource limits are not configured properly, slower builds are inevitable since jobs may slow down or fail. Also, coordinating scaling with multiple components like CI/CD tools and artifact storage is complicated. If not done properly, it can lead to bottlenecks and slow or failed builds too.

Also, it’s important to ensure all nodes in a Kubernetes cluster have the required dependencies and configurations to maintain consistent and reliable builds. If there are any inconsistencies, there will be build failures and errors. For example, different nodes can have different libraries or tool versions. It’s possible to manually update the dependencies across multiple nodes, but it increases the risk of errors if one node is out of sync. Even a small version mismatch can result in errors in large distributed systems.

When it comes to Docker containers, build engineers often rely on frozen images to ensure consistent environments. However, updating these images involves re-fetching and re-configuring them. Since each developer might have different local setups, this can be impractical. When pulling down an image, it's important to know what exact image you're going to use. Typically, developers would pull the latest image by using this command:

`docker pull ghcr.io/tracemachina/nativelink:latest`

The lack of image version specificity can lead to inconsistencies and complicate maintaining uniform Docker environments. Ensuring Docker images are consistent across all systems is a complex process. If the developer is not careful about these details, local environment inconsistencies can slow or break builds.

NativeLink Improves the Build Process
NativeLink is an open-source project that provides remote execution capabilities and hermetic builds across any system. This addresses the challenges discussed earlier on achieving efficient, consistent, and reproducible builds. NativeLink integrates with different build systems’ APIs like Bazel and Buck2 to provide remote build execution; artifacts like libraries and compiled binaries that are not available in the local Content Addressable Storage (CAS) within these build systems can be built with NativeLink.

NativeLink’s CAS is highly efficient as it stores data only once - data is stored based on the content's hash value rather than its location. For remote execution, NativeLink offloads build tasks to remote servers, significantly speeding up the build process. I encourage you to check out the details around how [NativeLink accelerated Samsung builds from 3 days to 1 hour](https://www.nativelink.com/resources/blog/case-study-samsung).

In short, NativeLink allows developers to share build caches and test builds on different architectures in the cloud without having access to the same infrastructure locally. Developers perform incremental builds in any environment because they pull non-changing artifacts from the CAS and use remote execution only on changing aspects of the code base.

## NativeLink Flexibility
There are 2 deployment options: on-prem and cloud. In either option, developers can use NativeLink without modifying their remote execution containers. Additionally, NativeLink operates independently of any autoscaling mechanisms where developers can implement any custom autoscaling solutions within their Kubernetes clusters.

## NativeLink’s Efficiency, Heremeticity, Reproducibility & Deployment
Running a build on a single CPU core takes a long time; using 200 cores speeds it up. However, this doesn't always translate to cost savings due to the nonlinear scalability of parallel tasks. NativeLink’s remote caching (CAS) skips unnecessary CPU cycles by reusing previously built artifacts. This speeds up the build process and also saves on CPU cycles by reusing previously built artifacts.

Compared to other projects, NativeLink is truly hermetic where any engineer can reproduce builds with near-perfect cache hit rates. This means that NativeLink is designed to create isolated, consistent build environments that ensure the exact same results every time a build is executed. This helps avoid issues related to environmental differences that often plague other build systems.

Deploying NativeLink doesn't require dependencies on the host system, unlike some other providers that may need a specific Python interpreter (i.e, Python 2 or Python 3). NativeLink is a single static executable, making it independent of the system's libc version. This allows for a minimalist setup where you can run NativeLink alongside a statically built compiler without needing an operating system in your container. Developers have the benefit of maintaining up-to-date environments without frequent rebuilds.

[NativeLink](https://github.com/TraceMachina/nativelink) is an active open source project built by Trace Machina, and you can fork it if you'd like to contribute or customize it for yourself . If you’re interested in learning more about NativeLink join me on the [NativeLink Community Slack](https://join.slack.com/t/nativelink/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A).
