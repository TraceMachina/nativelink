---
title: "Fine-tune an LLM on CPUs using Bazel and NativeLink"
tags: ["news", "blog-posts"]
image: https://github.com/user-attachments/assets/ddfb5684-327b-4af9-9618-be707eab894f
slug: finetune-llm-with-bazel-nativelink
pubDate: 2025-05-05
readTime: 20 minutes
---

## **Introduction**

The future of AI development belongs not necessarily to those with the most powerful infrastructure but to those who can extract maximum value from available resources. This tutorial emphasizes CPU-based fine-tuning demonstrating that with intelligent resource management through NativeLink, impressive results can be achieved without expensive GPU or TPU infrastructure. As compute becomes increasingly costly, competitive advantage will shift toward teams that optimize resource efficiency rather than those deploying state-of-the-art hardware.

This guide demonstrates how to establish an optimized AI development pipeline by integrating several key technologies:<br>
1\. **Bazel Build System**: For efficient repository management. A repository managed by Bazel allows your team to work in a unified codebase while maintaining clean separation of concerns.<br>
2\. **NativeLink**: A remote execution system hosted in your cloud. With NativeLink's remote execution capabilities, you can leverage your cloud resources optimally without wasteful duplication of work.<br>
3\. **Hugging Face Transformers**: For integrating with the rich ecosystem of open-source models that you can run locally or deploy anywhere. The transformers library also provides a sophisticated caching mechanism for optimizing loading model weights.<br>


## **Setting Up Your Repository**

Bazel is a build system designed for repositories that allows you to organize code into logical components while maintaining dependency relationships. For AI workloads, this is particularly valuable as it lets you separate model definitions, data processing pipelines, training code, and inference services.

### Prerequisites

1\. Bazel ([installation instructions](https://bazel.build/install)). This demo uses Bazel 8.1.1

2\. NativeLink [Cloud Account](https://app.nativelink.com/) (it’s free to get started, and secure) or [NativeLink 0.6.0](https://github.com/TraceMachina/nativelink/releases/tag/v0.6.0) (Apache-licensed, open source is hard-mode)

### Initial Setup

First, let's download all the files. From the folder where you want to download the files, run the following commands:

<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```bash
# Clone the entire repository
git clone https://github.com/TraceMachina/nativelink-blogs.git

# Navigate to the subdirectory
cd nativelink-blogs/finetuning_on_cpu
```
</div>


This will download all the files we need. Your `finetuning_on_cpu` repository should now have the following structure:
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```
|-finetuning_on_cpu
  |  |-.gitignore
  |  |-bazel_requirements_lock.txt
  |  |-MODULE.bazel
  |  |-python_provider.sh
  |  |-BUILD
  |  |-src
  |  |  |-training
  |  |  |  |-BUILD
  |  |  |  |-train_model.py
```
</div>

Here's what these files do:<br>
1\. `.gitignore` - Prevents unnecessary files from being committed to Git, keeping the repository clean<br>
2\. `bazel_requirements_lock.txt` - Ensures consistent Python dependencies across all environments<br>
3\. `MODULE.bazel` - Configures the project as a Bazel module, tells Bazel we'll need python and pip, and manages external dependencies<br>
4\. `python_provider.sh` - Our universal shell script that executes Python modules in the target environment to avoid cross-architecture problems (more on this below)<br>
5\. `BUILD` (root) - Exports common targets and tools used throughout the project<br>
6\. `src/training/BUILD` - Defines the model training targets and their dependencies<br>
7\. `src/training/train_model.py` - Main script that handles fine-tuning of language models on CPU using efficient training techniques<br>

## **Configuring Remote Execution With NativeLink**

### Prologue - Docker Image For Remote Execution

By default, NativeLink provides a minimal Ubuntu 22.04 image *without any* dependencies installed for remote execution. For this project, we created our own custom publicly-accessible docker image (Docker Hub reference:
`https://hub.docker.com/layers/evaanahmed2001/python-bazel-env/amd64-v2/images/sha256-6d8058b6b44ee34860297321f62b2fe99afae21c8594d499998105b3b699c9dd`) and we’ve made this free to use.

Alternatively, you can also create your own docker image from the `bazel_requirements_lock.txt` but this will be a time-consuming process. For context, creating this image on an M1 Pro MacBook took 4.5 hrs.

### Back to NativeLink

You’ll need to create `.bazelrc` file in the project root add your NativeLink credentials.<br>

1\. First create an empty .bazelrc file:<br>
```bash
# From nativelink-blogs/finetuning_on_cpu, run the following:
touch .bazelrc
```

2\. paste the following into your `.bazelrc` file:

<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```python
build --remote_cache=grpcs://cas-<your_nativelink_username>.build-faster.nativelink.net
build --remote_header=x-nativelink-api-key=<your_nativelink_api_key>
build --bes_backend=grpcs://bes-<your_nativelink_username>.build-faster.nativelink.net
build --bes_header=x-nativelink-api-key=<your_nativelink_api_key>
build --bes_results_url=https://app.nativelink.com/<your_nativelink_assigned_url>
build --remote_timeout=600

build --remote_executor=grpcs://scheduler-<your_nativelink_username>.build-faster.nativelink.net:443
build --remote_default_exec_properties=<your_docker_image>
# Alternatively, use our Docker Image via:
# build --remote_default_exec_properties="container-image=docker://docker.io/evaanahmed2001/python-bazel-env@sha256:8de13199d587964b218c0b671272b42031cf4944b2f426e6eee7d7542802bf7c"

test --test_verbose_timeout_warnings
test --test_output=all
```
</div>

<br>
Now replace
&lt;your_nativelink_username&gt;,
&lt;your_nativelink_api_key&gt;,
&lt;your_nativelink_assigned_url&gt;, and
&lt;your_docker_image&gt; with the actual values.
<br>
<br>
Regarding the first 5 build configurations above, NativeLink Cloud users can go to their Account > Quickstart and paste content from the Bazel section. Here’s a screenshot:
<br>
<br>
<img src="https://github.com/user-attachments/assets/1730c51d-c622-4865-a100-110b55b5180a" width=1000 alt="NativeLink Cloud Quickstart">

## **Important Note About Bazel’s Remote Execution Support**

Bazel supports remote execution for building (compiling) and testing, but `bazel run` uses the host platform as its target platform, meaning the executable will be invoked locally rather than on remote machines. This creates chip architecture compatibility issues - we can't build a file locally on an Apple Silicon Mac (ARM64) and run it on most cloud servers (AMD64). To execute binaries on remote servers, a workaround is to design tests that execute the binary, effectively leveraging `bazel test` which runs on the target platform.

Three of the most popular testing methods in Bazel are `py_test`, `native_test`, and `sh_test`:<br>
1\. `py_test` uses the `pytest` package to create tests (convenient, user-friendly)<br>
2\. `native_test` executes binaries directly without shell interpretation, making it efficient but more dependent on specific platform capabilities than shell-based tests.<br>
3\. `sh_test` involves coming up with a shell script that executes the test file (old-school but highly cross-platform and robust)<br>

When building tests with `py_test`, Bazel would build the tests locally and then execute them in the remote environment. This paves the way for architectural mismatch discussed above. This is why this demo uses shell script tests, or `sh_test`.

We use a generic `python_provider.sh` script in the root directory that loads Python from the execution environment and runs the specified Python module or script. In the `BUILD` files, `sh_test` rules reference this shared script and pass the Python module path as an argument. The test executes `python_provider.sh` which in turn executes the target Python file using the appropriate Python interpreter for the execution environment.

**DRAWBACK:**

Logging and print statements that track progress (like "Starting to fine tune model" or "Exiting this function") behave differently in remote execution. Unlike local runs where these appear in real-time, remote testing collects all logs on the server and only displays them after test completion when control returns to the local machine. The expected output is still fully preserved - just delayed until the process finishes running.

Let’s move on to the code.


## **1) Training / Fine-Tuning On NativeLink Cloud**

One of the key advantages of our setup is the ability to leverage powerful cloud resources through NativeLink's remote execution capabilities. This demo uses CPUs for fine-tuning. To observe the before-and-after effects of fine-tuning the lightweight model `"prajjwal1/bert-tiny"` for just 3 minutes on our cloud (targeting text sentiment analysis), run `train_model.py`

For local execution, use: <br>
`bazel run //src/training:train_model`

For remote execution, use: <br>
`bazel test //src/training:training_test`

### The NativeLink Difference:

To demonstrate NativeLink’s efficacy, consistency, and reliability, we ran the same fine-tuning job on the CPU of an M1 Pro MacBook Pro, the free version of Google Colab on CPU, and [NativeLink](https://github.com/TraceMachina/nativelink),which is free and open-source. We executed the fine-tuning task 5 times and this is what we observed:

1\. The Mac: the quickest run took 18 minutes while the slowest/longest took 20 minutes

2\. Free version of Google Colab: the quickest run took 10 minutes while the slowest/longest took 20 minutes. The execution time was widely varied. We suspect varying traffic on Google’s servers and how Colab allocates its compute resources played a part in this variability.

3\. Free NativeLink: the quickest run took 2 minutes while the slowest/longest took 3 minutes. NativeLink Cloud provided the quickest execution times by far.

<img src="https://github.com/user-attachments/assets/acf21144-b160-447f-846b-c516f0a250e4" width="1000" alt="Model Fine-Tuning Times">


## **Conclusion: Optimizing AI Development Through Resource Efficiency**

As demonstrated through this tutorial, the integration of Bazel's repository management, NativeLink's CPU-optimized remote execution, and Hugging Face's transformers library creates a development ecosystem that prioritizes computational efficiency over raw processing power. This approach addresses several critical challenges facing modern AI teams:

1\. **Resource Optimization**: By leveraging NativeLink's intelligent scheduling and optimization on CPU infrastructure, teams can achieve impressive fine-tuning results without the capital expenditure of specialized GPU/TPU hardware.<br>
2\. **Strategic Advantage**: This CPU-focused approach provides a competitive edge through efficient resource utilization, enabling teams to allocate budget toward innovation rather than hardware acquisition.<br>
3\. **Sustainable Scaling**: As models grow in size and complexity, the ability to efficiently distribute workloads across existing CPU infrastructure provides a more sustainable path to scale than continuously upgrading to the latest accelerators.<br>

For forward-thinking AI teams, this infrastructure stack represents a shift from the "bigger is better" hardware arms race toward thoughtful resource utilization. The competitive advantage increasingly belongs to those who can extract maximum value from available compute rather than those who deploy more powerful hardware.


The journey from experimental AI projects to production-grade systems demands both technical sophistication and resource awareness. By adopting this CPU-optimized approach with Bazel and NativeLink, your team can focus less on infrastructure limitations and more on the creative potential of fine-tuned models—developing applications that deliver genuine value while maintaining computational efficiency.
