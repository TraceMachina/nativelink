---
title: "Footprints of a Customer Service AI Agent That Reliably Decides to Request Human Intervention"
tags: ["news", "blog-posts"]
image: https://cdn.pixabay.com/photo/2023/05/09/20/36/ai-generated-7982424_1280.jpg
slug: customer-service-ai-agent
pubDate: 2025-05-02
readTime: 1 hour
---

## **Introduction**

The landscape of AI development is evolving rapidly. As organizations move beyond proof-of-concept AI applications to production-scale systems, they face unprecedented challenges in development velocity, infrastructure costs, and engineering complexity. Today's most ambitious AI teams are no longer just consuming models—they're building homegrown agents and agentic experiences that require sophisticated infrastructure.

This tutorial will guide you through setting up a state-of-the-art AI development environment that combines several powerful tools:

- **Anthropic's Model Context Protocol**: For structured communication with Claude and other AI models
- **Bazel Build System**: For efficient monorepo management
- **NativeLink**: A remote execution and caching system hosted in your cloud
- **Hugging Face Transformers**: For integrating with the rich ecosystem of open-source models that you can run locally or deploy anywhere

By the end of this tutorial, you'll have a foundation for an AI development workflow that can scale with your ambitions, whether you're a startup or an enterprise team looking to build the next generation of AI applications.

### Video Walk Through

A video walk through of this blog is available at:

https://www.youtube.com/watch?v=dmVyRz59m0o&list=PLu4lTD4juMi6ll8jaYHbBp0pM4-RSPEQ8

## **Why This Stack Matters**

<img src="https://img.notionusercontent.com/s3/prod-files-secure%2F448d4308-e035-401c-9af7-c04687aca8e5%2F9b85d48b-c159-4152-9167-dd17072e3dec%2FBeige_Minimal_Flowchart_Infographic_Graph.png/size/w=1150?exp=1746391380&sig=ss1WaoCmDlTCTOf38jULy4cpypJBl2YcEL5KgE4wwCw&id=1d6dbbd6-a6bd-80f3-b91a-e5bec009d229&table=block" width="1000" alt="Minimal Flowchart Graph">

Before diving into the technical details, let's understand why this particular combination of tools represents a step change for AI development:

1\. **Computational Efficiency**: With NativeLink's remote execution capabilities, you can leverage your cloud resources optimally, including Nvidia GPUs, without wasteful duplication of work.

2\. **Developer Experience**: A monorepo managed by Bazel allows your team to work in a unified codebase while maintaining clean separation of concerns.

3\. **Model Flexibility**: By incorporating both Anthropic's Claude and Hugging Face's ecosystem, you maintain flexibility to use the right model for each specific need.

Let's begin building our AI development environment.

## **Setting Up Your Monorepo with Bazel**

Bazel is a build system designed for mono-repos that allows you to organize code into logical components while maintaining dependency relationships. For AI workloads, this is particularly valuable as it lets you separate model definitions, data processing pipelines, training code, and inference services.

### Prerequisites

1\. Bazel ([installation instructions](https://bazel.build/install)). This demo uses Bazel 8.1.1

2\. NativeLink [Cloud Account](https://app.nativelink.com/) (it’s free to get started, and secure) or [NativeLink 0.6.0](https://github.com/TraceMachina/nativelink/releases/tag/v0.6.0) (Apache-licensed, open source is hard-mode)

3\. Anthropic API Key

### Initial Setup

First, let's create our monorepo and structure it. Let’s name our repository ai-monorepo.
1\. Create a directory called `ai-monorepo` and open your IDE from within it. Also create a `setup.sh` script. Alternatively, you can use the following shell commands
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```bash
# First cd into your preferred location
mkdir ai-monorepo
cd ai-monorepo
touch setup.sh
```
</div>

2\. Paste the following code in `setup.sh`:

<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```bash
#!/bin/bash
# Script to create the AI Monorepo project structure

set -e  # Exit on any error

echo "Creating AI Monorepo directory structure..."

# Create top-level files
touch bazel_requirements_lock.txt
touch .bazelrc
touch .gitignore
touch BUILD
touch MODULE.bazel

# Create src directory and subdirectories
mkdir -p src/{agents,cache_manager,demo,models,training}

# Create src/agents subdirectories
mkdir -p src/agents/{customer_service,hybrid_assistant,hybrid_customer_service}

# Create src/models subdirectories
mkdir -p src/models/{claude_client,huggingface}

# Create files in src/models/claude_client
touch src/models/claude_client/BUILD
touch src/models/claude_client/client.py
touch src/models/claude_client/run_claude_client.sh
touch src/models/claude_client/event_example.json
touch src/models/claude_client/sentiment_example.json

# Create files in src/models/huggingface
touch src/models/huggingface/BUILD
touch src/models/huggingface/transformers_client.py
touch src/models/huggingface/run_hugging_face.sh

# Create files in src/agents/customer_service
touch src/agents/customer_service/BUILD
touch src/agents/customer_service/service_agent.py
touch src/agents/customer_service/run_customer_service_agent.sh
touch src/agents/customer_service/test_conversations.json

# Create files in src/agents/hybrid_assistant
touch src/agents/hybrid_assistant/BUILD
touch src/agents/hybrid_assistant/assistant.py
touch src/agents/hybrid_assistant/run_assistant.sh

# Create files in src/agents/hybrid_customer_service
touch src/agents/hybrid_customer_service/BUILD
touch src/agents/hybrid_customer_service/hybrid_service_agent.py
touch src/agents/hybrid_customer_service/run_hybrid_customer_service_agent.sh
touch src/agents/hybrid_customer_service/test_conversations.json

# Create files in src/cache_manager
touch src/cache_manager/BUILD
touch src/cache_manager/cache_manager.py
touch src/cache_manager/run_cache_manager.sh

# Create files in src/demo
touch src/demo/BUILD
touch src/demo/complete_agent.py
touch src/demo/run_complete_agent.sh
touch src/demo/sample_context.txt

# Create files in src/training
touch src/training/BUILD
touch src/training/train_model.py
touch src/training/run_training.sh

# Make all shell scripts executable
find . -name "*.sh" -exec chmod +x {} \;
echo "Made all shell scripts executable"

echo "AI Monorepo directory structure created successfully!"
echo "Directory structure:"
find . -type f | sort

# Create a simple .env file template
cat > .env << EOF
# API keys
ANTHROPIC_API_KEY=your_api_key_here # Replace with your key enclosed in double quotes

# Test settings
RUN_API_TESTS=true
EOF

echo "Don't forget to update the .env file with your API keys!"
echo "Setup complete! Your AI Monorepo is ready for development."

```
</div>

3\. Run the following the commands:
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```bash
chmod +x setup.sh # Make this script an executable
./setup.sh # Run the script
```
</div>

This will create all the files we need in one go and we’ll add content along the way. Your ai-monorepo should now have the following structure:
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```
|-ai-monorepo
  |  |-setup.sh
  |  |-.bazelrc
  |  |-.env
  |  |-.gitignore
  |  |-BUILD
  |  |-MODULE.bazel
  |  |-bazel_requirements_lock.txt
  |  |-src
  |  |  |-agents
  |  |  |  |-customer_service
  |  |  |  |  |-BUILD
  |  |  |  |  |-run_customer_service_agent.sh
  |  |  |  |  |-service_agent.py
  |  |  |  |  |-test_conversations.json
  |  |  |  |-hybrid_assistant
  |  |  |  |  |-BUILD
  |  |  |  |  |-assistant.py
  |  |  |  |  |-run_assistant.sh
  |  |  |  |-hybrid_customer_service
  |  |  |  |  |-BUILD
  |  |  |  |  |-hybrid_service_agent.py
  |  |  |  |  |-run_hybrid_customer_service_agent.sh
  |  |  |  |  |-test_conversations.json
  |  |  |-cache_manager
  |  |  |  |-BUILD
  |  |  |  |-cache_manager.py
  |  |  |  |-run_cache_manager.sh
  |  |  |-demo
  |  |  |  |-BUILD
  |  |  |  |-complete_agent.py
  |  |  |  |-run_complete_agent.sh
  |  |  |  |-sample_context.txt
  |  |  |-models
  |  |  |  |-claude_client
  |  |  |  |  |-BUILD
  |  |  |  |  |-client.py
  |  |  |  |  |-event_example.json
  |  |  |  |  |-run_claude_client.sh
  |  |  |  |  |-sentiment_example.json
  |  |  |  |-huggingface
  |  |  |  |  |-BUILD
  |  |  |  |  |-run_hugging_face.sh
  |  |  |  |  |-transformers_client.py
  |  |  |-training
  |  |  |  |-BUILD
  |  |  |  |-run_training.sh
  |  |  |  |-train_model.py
```
</div>


Now, let’s set up the `bazel_requirements_lock` file. This file lists all the package with their versions needed for this demo. Paste the dependencies from the `bazel_requirements_lock.txt` below into your `bazel_requirements_lock.txt`:
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```
#
# This file is autogenerated by pip-compile via the following command:
#    pip-compile --output-file=bazel_requirements_lock.txt requirements.txt
#
aiohappyeyeballs==2.6.1
    # via aiohttp
aiohttp==3.11.14
    # via
    #   datasets
    #   fsspec
aiosignal==1.3.2
    # via aiohttp
async-timeout==5.0.1
    # via aiohttp
attrs==25.3.0
    # via aiohttp
certifi==2025.1.31
    # via requests
charset-normalizer==3.4.1
    # via requests
datasets==3.4.1
    # via -r requirements.txt
dill==0.3.7
    # via
    #   datasets
    #   multiprocess
filelock==3.18.0
    # via
    #   huggingface-hub
    #   torch
    #   transformers
frozenlist==1.5.0
    # via
    #   aiohttp
    #   aiosignal
fsspec[http]==2024.12.0
    # via
    #   datasets
    #   huggingface-hub
    #   torch
huggingface-hub==0.29.3
    # via
    #   datasets
    #   tokenizers
    #   transformers
idna==3.10
    # via
    #   requests
    #   yarl
jinja2==3.1.6
    # via torch
markupsafe==3.0.2
    # via jinja2
mpmath==1.3.0
    # via sympy
multidict==6.2.0
    # via
    #   aiohttp
    #   yarl
multiprocess==0.70.15
    # via datasets
networkx==3.2.1
    # via torch
numpy==1.26.4
    # via
    #   datasets
    #   pandas
    #   transformers
packaging==24.2
    # via
    #   datasets
    #   huggingface-hub
    #   transformers
pandas==2.2.3
    # via datasets
propcache==0.3.0
    # via
    #   aiohttp
    #   yarl
pyarrow==19.0.1
    # via datasets
python-dateutil==2.9.0.post0
    # via pandas
pytz==2025.2
    # via pandas
pyyaml==6.0.2
    # via
    #   datasets
    #   huggingface-hub
    #   transformers
regex==2024.11.6
    # via transformers
requests==2.32.3
    # via
    #   datasets
    #   huggingface-hub
    #   transformers
safetensors==0.5.3
    # via transformers
six==1.17.0
    # via python-dateutil
sympy==1.13.1
    # via torch
tokenizers==0.21.1
    # via transformers
setuptools==69.0.2
    # via torch
torch==2.6.0
    # via -r requirements.txt
tqdm==4.67.1
    # via
    #   datasets
    #   huggingface-hub
    #   transformers
transformers==4.50.1
    # via -r requirements.txt
typing-extensions==4.12.2
    # via
    #   huggingface-hub
    #   multidict
    #   torch
tzdata==2025.2
    # via pandas
urllib3==2.3.0
    # via requests
xxhash==3.5.0
    # via datasets
yarl==1.18.3
    # via aiohttp
accelerate==1.5.2
psutil==5.9.8
    # for accelerate
anthropic==0.49.0
anyio==4.9.0
    # for anthropic
distro==1.9.0
    # for anthropic
httpx==0.28.1
    # for anthropic
jiter==0.9.0
    # for anthropic
pydantic-core==2.33.0
    # for anthropic
pydantic==2.11.0
    # for anthropic
sniffio==1.3.1
    # for anthropic
httpcore==1.0.7
    # for anthropic
h11==0.14.0
    # for anthropic
annotated-types==0.7.0
    # for anthropic
typing-inspection==0.4.0
    # for anthropic
exceptiongroup==1.2.2
    # for anthropic
python-dotenv==1.1.0
    # for loading env variables
scipy==1.15.2
    # for torch to get embeddings
```
</div>


Now, let’s set up the `.gitignore` file in case you use git for this demo (recommended). Paste the following into your `.gitignore`:

<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```
.DS_Store # only for macs
bazel-ai-monorepo
bazel-out
bazel-bin
bazel-testlogs
.bazelrc
.env
```
</div>

Next, let’s set up your local environment.

1\. First, let’s populate the `.env` file. Paste the following into it:

    ```
    ANTHROPIC_API_KEY=your_anthropic_api_key # Replace with your key enclosed in double quotes
    RUN_API_TESTS=true

    ```

2\. Then add the following into the project root’s `BUILD` file:

    ```bash
    # Root BUILD file
    exports_files([".env"], visibility=["//visibility:public"])
    # This makes the .env all accessible to every file in the repo
    ```


Lastly, we’ll let Bazel know which python version we want and how to resolve dependencies (we’ll use the default pip). Paste the following into your root’s `MODULE.bazel` file:
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```python
bazel_dep(name = "rules_python", version = "1.2.0")

python = use_extension("@rules_python//python/extensions:python.bzl", "python")
python.toolchain(
    python_version = "3.13",
)

pip = use_extension("@rules_python//python/extensions:pip.bzl", "pip")
pip.parse(
    hub_name = "pypi",
    python_version = "3.13",
    requirements_lock = "//:bazel_requirements_lock.txt",
)

use_repo(pip, "pypi")
```
</div>

## **Configuring Remote Execution With NativeLink**

### Prologue - Docker Image For Remote Execution

By default, NativeLink provides a minimal Ubuntu 22.04 image *without any* dependencies installed for remote execution. For this project, we created our own custom publicly-accessible docker image (Docker Hub reference: `"container-image=docker://docker.io/evaanahmed2001/python-bazel-env:amd64-v2”`) and we’ve made this free to use.

Alternatively, you can also create your own docker image from the `bazel_requirements_lock.txt` but this will be a time-consuming process. For context, creating this image on a M1 Pro MacBook took 4.5 hrs.

### Back to NativeLink

You’ll need to add your NativeLink credentials into the `.bazelrc` file in the project root. First paste the following into your `.bazelrc` file:

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
# build --remote_default_exec_properties="container-image=docker://docker.io/evaanahmed2001/python-bazel-env:amd64-v2"

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
<img src="https://img.notionusercontent.com/s3/prod-files-secure%2F448d4308-e035-401c-9af7-c04687aca8e5%2Fe35aacb4-443d-4aac-ae80-519a3953e4b2%2FScreenshot_2025-04-08_at_7.39.37_PM.png/size/w=1420?exp=1746391457&sig=PvRgFNAStoMSXHAlu9LPhTRZbyX6GqDjaiSSAPgsaxg&id=1d6dbbd6-a6bd-8056-b0cd-c7b324f6ab7c&table=block" width=1000 alt="NativeLink Cloud Quickstart">

## **Important Note About Bazel’s Remote Execution Support**

Bazel supports remote execution for building (compiling) and testing only, **NOT** for running the built executables. To use remote servers for remote build execution via Bazel, a roundabout way is to design tests that just execute the Python binary we want to run. If the binary terminates without any errors, the test passes.

Two of the most popular testing methods in Bazel are `py_test` and `sh_test`:

1\. `py_test` uses the `pytest` package to create tests (convenient, user-friendly)

2\. `sh_test` involves coming up with a shell script that executes the test file (old-school but functional and robust)

When building tests with `py_test`, Bazel would build the tests locally and then execute them in the remote environment. This paves the way for architectural mismatch. For example, NativeLink cloud computers use x86-64 chips while modern day Macs use ARM-64 ones, so a `py_test` compiled on Apple Silicon will never run on cloud computers (majority of them use x86 chips). This is why this demo uses shell script tests, or `sh_test`. For a python file named `<file_name>.py`, there’s a `run_<file_name>.sh` script which loads Python from the execution environment and runs `python3 <file_name>.py`.

The flow then boils down to creating a `sh_test` involving `run_<file_name>.sh` and running the test. The test would execute `run_<file_name>.sh` which in turn would execute `<file_name>.py`.

**DRAWBACK:**

Print statements that track progress (like "Starting to train model" or "Exiting this function") behave differently in remote execution. Unlike local runs where these appear in real-time, remote testing collects all logs on the server and only displays them after test completion when control returns to the local machine. The expected output is still fully preserved - just delayed until the process finishes running.

Let’s move on to the code.

## **1) Implementing Model Context Protocol with Claude**

<!-- vale Vale.Spelling = NO -->
Anthropic's Model Context Protocol allows for structured communication with Claude, making it easier to build reliable AI applications. Let's implement a basic client that uses this protocol under `src/models/claude_client`:

- `client.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">



    ```python
    # In src/models/claude_client/client.py

    from anthropic import Anthropic

    import json
    import os
    from dotenv import load_dotenv

    class ClaudeClient:
        def __init__(self, api_key=None):
            self.client = Anthropic(api_key=api_key)

        def get_structured_response(self, system_prompt, user_message, response_schema):
            """
            Get a structured response from Claude using the Model Context Protocol

            Args:
                system_prompt: Instructions for Claude
                user_message: The user's input
                response_schema: JSON schema defining the expected response format

            Returns:
                Structured response conforming to the schema
            """

            # Define JSON tool with the provided schema
            tools = [
                {
                    "name": "structured_response",
                    "description": "Returns a structured JSON response",
                    "input_schema": response_schema,
                },
            ]

            response = self.client.messages.create(
                model="claude-3-haiku-20240307",
                system=system_prompt,
                messages=[{"role": "user", "content": user_message}],
                max_tokens=1024,
                temperature=0,
                tools=tools,
                tool_choice={"type": "tool", "name": "structured_response"},
            )

            # Extract the structured response from the tool calls
            if response.content:
                for content_block in response.content:
                    if (
                        hasattr(content_block, "type")
                        and content_block.type == "tool_use"
                        and content_block.name == "structured_response"
                    ):
                        return content_block.input

            # If no tool use was found, just return None with a print warning
            text_response = None
            if response.content:
                for content_block in response.content:
                    if hasattr(content_block, "type") and content_block.type == "text":
                        text_response = content_block.text
                        break

            print(
                "WARNING: Claude didn't use the provided schema and responded with text instead."
            )
            if text_response:
                print(f"Claude's text response: {text_response[:100]}...")

            return None  # Return None to indicate no structured response

    if __name__ == "__main__":

        # Load environment variables from .env file (just one line!)
        load_dotenv()
        # Get API key from environment
        anthropic_api_key = os.getenv("ANTHROPIC_API_KEY")
        run_api_test = os.getenv("RUN_API_TESTS", "False").lower() == "true"

        if not anthropic_api_key:
            raise ValueError(
                "ANTHROPIC_API_KEY not found in environment variables. Please check your .env file."
            )

        if run_api_test == False:
            print(
                "Skipping tests that use Anthropic's API since RUN_API_TESTS is set to false in the .env file at the project's root."
            )
            exit()

        MyClaudeClient = ClaudeClient(api_key=anthropic_api_key)

        example_files = ["sentiment_example.json", "event_example.json"]
        for example_file in example_files:
            try:
                # Get the file path relative to the current script
                file_path = os.path.join(
                    os.path.dirname(os.path.abspath(__file__)), example_file
                )

                # Load example data
                with open(file_path, "r") as f:
                    example_data = json.load(f)

                system_prompt = example_data["system_prompt"]
                user_message = example_data["user_message"]
                response_schema = example_data["response_schema"]

                print(f"\nProcessing example: {os.path.basename(example_file)}")

                # Get structured response
                result = MyClaudeClient.get_structured_response(
                    system_prompt, user_message, response_schema
                )

                print(f"Result:")
                if result:
                    print(json.dumps(result, indent=2))
                else:
                    print("No structured response received")

            except FileNotFoundError:
                print(f"Example file not found: {example_file}")
            except json.JSONDecodeError:
                print(f"Invalid JSON in example file: {example_file}")
            except Exception as e:
                print(f"Error processing example {example_file}: {e}")

            print("-" * 50)

    ```
    </div>

- `event_example.json -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```json
    {
        "system_prompt": "You are an event information extraction assistant. Extract key details from the event description and return them in a structured format.",
        "user_message": "Join us for TechConf 2025, the premier technology conference happening in San Francisco from April 15-17, 2025. Early bird tickets are $399 until March 15, then regular admission is $599. Featured speakers include Dr. Sarah Chen (AI Ethics), Mark Johnson (Quantum Computing), and Lisa Rodriguez (Cybersecurity). The conference will be held at the Moscone Center. Register at techconf2025.com.",
        "response_schema": {
            "type": "object",
            "properties": {
                "event_name": {
                    "type": "string"
                },
                "location": {
                    "type": "string"
                },
                "start_date": {
                    "type": "string",
                    "format": "date"
                },
                "end_date": {
                    "type": "string",
                    "format": "date"
                },
                "ticket_prices": {
                    "type": "object",
                    "properties": {
                        "early_bird": {
                            "type": "number"
                        },
                        "regular": {
                            "type": "number"
                        }
                    }
                },
                "speakers": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string"
                            },
                            "topic": {
                                "type": "string"
                            }
                        }
                    }
                },
                "venue": {
                    "type": "string"
                },
                "website": {
                    "type": "string"
                }
            },
            "required": [
                "event_name",
                "location",
                "start_date",
                "end_date",
                "ticket_prices",
                "speakers"
            ]
        }
    }

    ```
    </div>

- `sentiment_example.json -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```json
    {
        "system_prompt": "You are a sentiment analysis assistant. Analyze the provided text and return a structured response with the sentiment (positive, negative, or neutral) and confidence score.",
        "user_message": "I absolutely loved the new restaurant we tried last night! The food was amazing and the service was excellent.",
        "response_schema": {
            "type": "object",
            "properties": {
                "sentiment": {
                    "type": "string",
                    "enum": [
                        "positive",
                        "negative",
                        "neutral"
                    ]
                },
                "confidence": {
                    "type": "number",
                    "minimum": 0,
                    "maximum": 1
                },
                "explanation": {
                    "type": "string"
                }
            },
            "required": [
                "sentiment",
                "confidence",
                "explanation"
            ]
        }
    }
    ```
    </div>

- `run_claude_client.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting Claude Client test from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script
    echo "Running Python script ..."
    python3 "$DIR/client.py"

    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    </div>

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="claude_client",
        srcs=["client.py"],
        main="client.py",
        data=[
            "sentiment_example.json",
            "event_example.json",
            "//:.env",  # Add your .env file here
        ],
        deps=[
            "@pypi//anthropic",
            "@pypi//python_dotenv",
        ],
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="claude_client_test",
        srcs=["run_claude_client.sh"],
        data=[
            ":claude_client",
        ],
    )

    ```
    </div>


The code in the main block uses the ClaudeClient class to get MCP compliant responses from Claude on 2 example queries (stored as JSON files in this sub-directory). See the results by running client.py.

For local execution, use: <br>
`bazel run //src/models/claude_client:claude_client`

For remote execution, use: <br>
`bazel test //src/models/claude_client:claude_client_test`

## **2) An Application of Claude With MCP - Consistent Customer Service AI Agent**

<img src="https://img.notionusercontent.com/s3/prod-files-secure%2F448d4308-e035-401c-9af7-c04687aca8e5%2F5c8f6f49-beba-4845-be2d-2a0fe6593262%2FProcess_of_Creative_Thinking_Flowchart_Graph.png/size/w=2000?exp=1746313235&sig=rFVGmMeiZ8neB-SCSEZMV_R7sdfN8ULz9ITZagfgyO0&id=1d6dbbd6-a6bd-80b0-83db-d8f77ea8e393" width="1000" alt="Decision Flow for Seeking Human Intervention">

<!-- vale Vale.Spelling = YES -->

We can use MCP to consistently get boolean responses from Claude which can trigger certain actions. Moreover, setting temperature to 0 further increases consistency/predictability. We can leverage this to devise a customer service agent that we can program to reliably seek human intervention under similar conditions. Testing shows that when evaluating customer queries with clear emotion signals, the agent makes identical transfer decisions in repeated trials, ensuring that critical escalation points aren't subject to the statistical variance that typically affects large language model outputs. The primary objective is to achieve predictable behavior rather than optimal performance—specifically, ensuring that the model's decision patterns remain consistent across identical inputs, whether those decisions are correct or incorrect.

- `service_agent.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```python
    import os
    import sys
    import json
    from typing import List, Dict, Tuple, Optional, Any
    from enum import Enum
    from dataclasses import dataclass

    # Add root directory to Python path for absolute imports
    sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../../..")))

    # Try both import styles for compatibility
    try:
        # Try absolute import first
        from src.models.claude_client.client import ClaudeClient
    except ImportError:
        # Fall back to relative import if absolute fails
        from ...models.claude_client.client import ClaudeClient

    @dataclass
    class ConversationMessage:
        """Data class representing a single message in a conversation"""

        sender: str  # "customer" or "agent"
        content: str
        timestamp: str = ""
        metadata: Dict[str, Any] = None

        def __post_init__(self):
            """Initialize default values for optional fields"""
            import datetime

            if not self.timestamp:
                self.timestamp = datetime.datetime.now().isoformat()
            if self.metadata is None:
                self.metadata = {}

    @dataclass
    class Conversation:
        """Data class representing a customer service conversation"""

        conversation_id: str
        history: List[ConversationMessage] = None
        metadata: Dict[str, Any] = None

        def __post_init__(self):
            """Initialize default values for optional fields"""
            if self.history is None:
                self.history = []
            if self.metadata is None:
                self.metadata = {}

        def add_message(
            self, sender: str, content: str, metadata: Dict[str, Any] = None
        ) -> None:
            """Add a message to the conversation history"""
            if metadata is None:
                metadata = {}

            message = ConversationMessage(sender=sender, content=content, metadata=metadata)

            self.history.append(message)

        def get_conversation_text(self) -> str:
            """Get the full conversation as a formatted text string"""
            text = ""
            for message in self.history:
                prefix = "Customer: " if message.sender == "customer" else "Agent: "
                text += f"{prefix}{message.content}\n\n"
            return text

    class CustomerServiceAgent:
        """
        Customer service agent that uses a light-weight Hugging Face model
        for responses and Claude MCP for transfer decisions
        """

        def __init__(
            self,
            anthropic_api_key: str,
        ):
            """
            Initialize the customer service agent

            Args:
                anthropic_api_key: API key for Anthropic's Claude
            """
            self.anthropic_api_key = anthropic_api_key
            # Initialize Claude client for MCP
            self.claude_client = ClaudeClient(api_key=anthropic_api_key)

        def evaluate_transfer_need(
            self, conversation: Conversation, verbose=False
        ) -> Dict[str, Any]:
            """
            Use Claude MCP to evaluate if a conversation needs human transfer

            Args:
                conversation: The customer conversation to evaluate

            Returns:
                Structured response with transfer decision and reasoning
            """
            # Define the response schema for Claude MCP
            response_schema = {
                "type": "object",
                "properties": {
                    "transfer_needed": {
                        "type": "boolean",
                        "description": "Whether this conversation should be transferred to a human agent",
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Detailed reasoning for the transfer decision",
                    },
                    "detected_emotion": {
                        "type": "object",
                        "properties": {
                            "primary_emotion": {
                                "type": "string",
                                "enum": [
                                    "anger",
                                    "anxiety",
                                    "frustration",
                                    "worry",
                                    "confusion",
                                    "neutral",
                                    "satisfaction",
                                    "urgency",
                                ],
                                "description": "Primary emotion detected in the customer's message",
                            },
                            "intensity": {
                                "type": "string",
                                "enum": ["low", "medium", "high"],
                                "description": "Intensity of the detected emotion",
                            },
                            "confidence": {
                                "type": "number",
                                "minimum": 0,
                                "maximum": 1,
                                "description": "Confidence in the emotion detection (0-1)",
                            },
                        },
                        "required": ["primary_emotion", "intensity", "confidence"],
                        "description": "Analysis of the customer's emotional state",
                    },
                },
                "required": ["transfer_needed", "reasoning", "detected_emotion"],
            }

            # Build the system prompt for Claude
            system_prompt = """
            You are an AI system that evaluates customer service conversations to determine if they should be transferred to a human agent.

            TRANSFER CRITERIA:
            1. Urgent issues that require immediate attention ("urgent", "immediately", "right now", "asap", etc.)
            2. Emotional distress (high anger, anxiety, frustration, or any negative emotion)
            3. Complex technical or account issues beyond AI capabilities
            4. Explicit requests for human assistance
            5. Security or privacy concerns
            6. Billing/payment disputes
            7. Escalating frustration across multiple messages
            8. Confusion that persists after clarification attempts

            Analyze the conversation carefully and provide structured output indicating whether transfer is needed, with detailed reasoning and emotional analysis.
            """

            # Get the conversation text for Claude to analyze
            conversation_text = conversation.get_conversation_text()
            if verbose:
                print("*" * 50)
                print(
                    "Going to send the following conversation to Claude MCP for evaluation:"
                )
                print(conversation_text)

            # Use Claude MCP to get a structured decision
            mcp_response = self.claude_client.get_structured_response(
                system_prompt=system_prompt,
                user_message=f"Please analyze this customer service conversation and determine if it should be transferred to a human agent:\n\n{conversation_text}",
                response_schema=response_schema,
            )

            if verbose:
                print("Claude MCP response:")
                print(json.dumps(mcp_response, indent=2))
                print("*" * 50)

            return mcp_response

        def generate_response(
            self, conversation: Conversation, is_transfer=False, verbose=False
        ) -> str:
            """
            For now, returns a placeholder response

            Returns:
                Generated response text
            """
            placeholder_response = "Ok, tell me more about your situation"
            return placeholder_response

        def process_message(
            self, conversation: Conversation, new_message: str = None, verbose=False
        ) -> Tuple[str, bool, Dict[str, Any]]:
            """
            Process a new customer message and determine if transfer is needed

            Args:
                conversation: The ongoing conversation
                new_message: New message from customer (if not already in history)

            Returns:
                Tuple of (response generated by Hugging Face model, needs_transfer, mcp_evaluation)
            """
            # Add the new message to conversation if provided
            if new_message:
                conversation.add_message("customer", new_message)

            # Use Claude MCP to evaluate transfer need
            mcp_evaluation = self.evaluate_transfer_need(conversation, verbose=verbose)

            # Check if we need to transfer
            needs_transfer = mcp_evaluation.get("transfer_needed", False)

            # Generate appropriate response with the Hugging Face model
            response = self.generate_response(
                conversation, is_transfer=needs_transfer, verbose=verbose
            )

            # Add the response to conversation history
            conversation.add_message("agent", response)

            return response, needs_transfer, mcp_evaluation

    def run_test_conversation(
        agent, conversation_messages, expected_transfer_point, verbose=False
    ):
        """
        Run a test conversation and check if transfer happens at the expected point

        Args:
            agent: The CustomerServiceAgent instance
            conversation_messages: List of customer messages to process sequentially
            expected_transfer_point: At which message transfer should occur (1-indexed)
            verbose: Whether to print detailed progress

        Returns:
            Tuple of (passed, actual_transfer_point, conversation)
        """
        conversation = Conversation(conversation_id=f"test-{id(conversation_messages)}")
        transfer_occurred = False
        actual_transfer_point = None

        if verbose:
            print(
                f"\n*** --- Expected transfer at message {expected_transfer_point} --- ***\n"
            )

        for i, message in enumerate(conversation_messages, 1):
            if verbose:
                print(f"\nCustomer (message {i}): {message}")

            response, needs_transfer, transfer_details = agent.process_message(
                conversation, message, verbose=False
            )  # setting verbose=true would print out all model responses every step of the way

            if verbose:
                print(f"Agent: {response}")
                print(f"Transfer needed: {needs_transfer}")
                if needs_transfer:
                    print(
                        f"\nReasoning: {transfer_details.get('reasoning', 'No reasoning provided')}"
                    )
                    emotion = transfer_details.get("detected_emotion", {})
                    print(
                        f"\nDetected emotion: {emotion.get('primary_emotion', 'unknown')} (intensity: {emotion.get('intensity', 'unknown')})"
                    )

            if needs_transfer and not transfer_occurred:
                transfer_occurred = True
                actual_transfer_point = i

                if verbose:
                    print(f"\n🔀 TRANSFER OCCURRED AT MESSAGE {i} 🔀")

                # No need to continue the conversation after transfer
                break

        test_passed = actual_transfer_point == expected_transfer_point

        if test_passed:
            print(
                f"\n✅ TEST PASSED: Transfer occurred at expected message {expected_transfer_point}"
            )
        else:
            print(
                f"\n❌ TEST FAILED: Expected transfer at message {expected_transfer_point}, but "
            )
            if actual_transfer_point:
                print(f"transfer occurred at message {actual_transfer_point}")
            else:
                print("no transfer occurred")

        print(f"{'='*80}")

        return test_passed, actual_transfer_point, conversation

    def run_multiple_tests(agent, file_path):

        # Get the file path relative to the current script
        test_file_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), file_path)

        # Load test conversations from JSON file
        try:
            with open(test_file_path, "r") as json_file:
                test_data = json.load(json_file)
        except FileNotFoundError:
            print("Error: test_conversations.json file not found")
            sys.exit(1)
        except json.JSONDecodeError:
            print("Error: Invalid JSON in test_conversations.json")
            sys.exit(1)

        # Run all tests 4 times to demonstrate reproducible results
        all_runs_results = []

        for run_num in range(1, 5):  # 4 runs
            print(f"\n{'*'*100}\nTEST RUN {run_num} OF 4\n{'*'*100}")

            # Create results dictionary to track outcomes for this run
            results = {"run": run_num, "pass": 0, "fail": 0, "tests": []}

            # Process each test group
            for group_name, conversations in test_data.items():
                print(f"\n{'='*80}\nTEST GROUP: {group_name}\n{'='*80}")

                for test_case in conversations:
                    test_id = test_case.get("id", "unknown")
                    expected_transfer = test_case.get("expected_transfer_point")
                    messages = test_case.get("messages", [])

                    print(f"\n{'-'*40}\nRunning test: {test_id}\n{'-'*40}")
                    test_passed, actual_transfer, conversation = run_test_conversation(
                        agent, messages, expected_transfer, verbose=True
                    )

                    # Track results
                    results["tests"].append(
                        {
                            "id": test_id,
                            "group": group_name,
                            "expected_transfer": expected_transfer,
                            "actual_transfer": actual_transfer,
                            "passed": test_passed,
                            "message_count": len(messages),
                        }
                    )

                    if test_passed:
                        results["pass"] += 1
                    else:
                        results["fail"] += 1

            # Store results for this run
            all_runs_results.append(results)

            # Summary for this run
            print(f"\n{'*'*100}\nRUN {run_num} RESULTS SUMMARY\n{'*'*100}")
            print(f"Total tests: {results['pass'] + results['fail']}")
            print(f"Passed: {results['pass']}")
            print(f"Failed: {results['fail']}")

            if results["fail"] > 0:
                print("\nFailed tests in this run:")
                for test in results["tests"]:
                    if not test["passed"]:
                        print(f"- {test['id']} (group: {test['group']})")
                        print(
                            f"  Expected transfer at message {test['expected_transfer']}, got {test['actual_transfer'] or 'no transfer'}"
                        )

        # Overall summary across all runs
        print(f"\n{'='*100}\nOVERALL RESULTS ACROSS ALL 4 RUNS\n{'='*100}")

        # Count total passes and fails
        total_tests = sum(run["pass"] + run["fail"] for run in all_runs_results)
        total_passes = sum(run["pass"] for run in all_runs_results)
        total_fails = sum(run["fail"] for run in all_runs_results)

        print(f"Total tests executed: {total_tests}")
        print(f"Total passes: {total_passes}")
        print(f"Total fails: {total_fails}")
        print(f"Overall pass rate: {(total_passes/total_tests)*100:.2f}%")

        # Check for consistency across runs
        print("\nConsistency analysis:")
        all_test_ids = {test["id"] for run in all_runs_results for test in run["tests"]}

        for test_id in sorted(all_test_ids):
            # Collect results for this test across all runs
            test_results = []
            for run in all_runs_results:
                for test in run["tests"]:
                    if test["id"] == test_id:
                        test_results.append(test)
                        break

            # Check if all runs produced the same result for this test
            if all(r["passed"] == test_results[0]["passed"] for r in test_results):
                result = (
                    "Consistent: All passed"
                    if test_results[0]["passed"]
                    else "Consistent: All failed"
                )
            else:
                result = "Inconsistent results"

            print(f"- {test_id}: {result}")
        return

    # Example usage
    if __name__ == "__main__":
        # Load API key from environment
        from dotenv import load_dotenv

        load_dotenv()

        anthropic_api_key = os.environ.get("ANTHROPIC_API_KEY")
        if not anthropic_api_key:
            print("Error: ANTHROPIC_API_KEY not found in environment")
            sys.exit(1)

        agent = CustomerServiceAgent(
            anthropic_api_key=anthropic_api_key,
        )

        # Example test conversation that should trigger transfer on 3rd message
        # test_conversation_1 = [
        #     "I have two accounts with your service. Can I merge them into one?",
        #     "Both accounts have subscription history and saved information I want to keep.",
        #     "I tried the account settings page, but there's no option to merge accounts there.",
        #     "This is beyond frustrating! I've been trying to solve this simple problem for DAYS! I need to speak to someone with actual authority who can merge these accounts IMMEDIATELY!",
        # ]
        # run_test_conversation(
        #     agent, test_conversation_1, expected_transfer_point=3, verbose=True
        # )

        # To run all the tests
        file_path = "test_conversations.json"
        run_multiple_tests(agent, file_path)

    ```
    </div>

- `test_conversations.json -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```json
    {
        "transfer_on_first_message": [
            {
                "id": "urgent-security-1",
                "expected_transfer_point": 1,
                "messages": [
                    "My account was just hacked! Someone is making purchases RIGHT NOW with my credit card!"
                ]
            },
            {
                "id": "urgent-security-2",
                "expected_transfer_point": 1,
                "messages": [
                    "I just got a fraud alert! Someone's using my account in another country RIGHT NOW!"
                ]
            },
            {
                "id": "urgent-payment-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I was just charged $2000 for something I didn't buy! This is URGENT!"
                ]
            },
            {
                "id": "extremely-angry-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I am FURIOUS with your company! I've been a customer for 10 years and this is OUTRAGEOUS! Fix this IMMEDIATELY!"
                ]
            },
            {
                "id": "critical-deadline-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I need IMMEDIATE assistance! My business account is down and I have a critical presentation in 30 minutes!"
                ]
            },
            {
                "id": "missing-order-urgent",
                "expected_transfer_point": 1,
                "messages": [
                    "I need URGENT help! My order for my wedding is missing and the ceremony is TOMORROW! I'm panicking!"
                ]
            },
            {
                "id": "legal-threat-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I'm contacting my lawyer immediately about this fraudulent charge! This is THEFT and I need to speak to a manager RIGHT NOW!"
                ]
            },
            {
                "id": "medical-urgency",
                "expected_transfer_point": 1,
                "messages": [
                    "I need urgent help! My medical device order hasn't arrived and I'm out of supplies. This is a MEDICAL EMERGENCY!"
                ]
            },
            {
                "id": "explicit-human-request-urgent",
                "expected_transfer_point": 1,
                "messages": [
                    "I need to speak to a HUMAN REPRESENTATIVE immediately! This is regarding an urgent security issue with my account!"
                ]
            },
            {
                "id": "extreme-distress",
                "expected_transfer_point": 1,
                "messages": [
                    "I'm literally shaking with anger right now! Your company has charged me THREE TIMES for the same order! I need this fixed IMMEDIATELY!"
                ]
            }
        ],
        "transfer_on_second_message": [
            {
                "id": "refund-escalation",
                "expected_transfer_point": 2,
                "messages": [
                    "I'd like to request a refund for my recent purchase. The product was defective.",
                    "I don't care about your policy! I've been waiting for TWO WEEKS and I demand a refund NOW! Get me a manager!"
                ]
            },
            {
                "id": "technical-issue-escalation",
                "expected_transfer_point": 2,
                "messages": [
                    "The app keeps crashing when I try to upload my documents. Is there a fix?",
                    "I've tried all your suggestions and NOTHING works! This is costing me business! I need to speak to your technical team immediately!"
                ]
            },
            {
                "id": "sudden-urgency-development",
                "expected_transfer_point": 2,
                "messages": [
                    "What are your business hours for customer support?",
                    "Actually, I just noticed someone has hacked my account! There are purchases I didn't make! I need urgent help RIGHT NOW!"
                ]
            },
            {
                "id": "second-message-human-request",
                "expected_transfer_point": 2,
                "messages": [
                    "Can you tell me how to update my profile information?",
                    "This is too complicated. I need to speak with a HUMAN representative immediately. Please connect me with someone who can help."
                ]
            },
            {
                "id": "health-concern-escalation",
                "expected_transfer_point": 2,
                "messages": [
                    "I'm trying to find information about potential allergens in your products.",
                    "This is a SERIOUS medical concern! I need to speak with someone immediately about this product - my child had a severe reaction and I need to know what's in it RIGHT NOW!"
                ]
            },
            {
                "id": "increasing-frustration-3-messages",
                "expected_transfer_point": 2,
                "messages": [
                    "I can't seem to download my invoice for last month's purchase. Where can I find it?",
                    "I checked there already, but the invoice isn't showing up. Can you send it directly?",
                    "This is RIDICULOUS! I've spent 30 minutes trying to get a simple invoice! I need to speak to someone competent IMMEDIATELY!"
                ]
            },
            {
                "id": "shipping-address-problem",
                "expected_transfer_point": 2,
                "messages": [
                    "How do I change the shipping address for my recent order?",
                    "I tried that but it says it's too late to modify the order since it's being processed.",
                    "This is an EMERGENCY! I won't be at that address and the package will be STOLEN! I need to speak to someone who can actually HELP ME!"
                ]
            },
            {
                "id": "warranty-claim-difficulty",
                "expected_transfer_point": 2,
                "messages": [
                    "How do I make a warranty claim for my broken headphones?",
                    "I filled out the form, but I haven't received any confirmation email yet.",
                    "It's been TWO DAYS and I've heard NOTHING! This is completely unprofessional! I want to speak to a manager about this IMMEDIATELY!"
                ]
            },
            {
                "id": "confused-to-angry-progression",
                "expected_transfer_point": 2,
                "messages": [
                    "I'm not sure I understand the difference between your Basic and Premium plans. Can you explain?",
                    "Thanks, but what exactly does 'priority support' mean in practical terms?",
                    "Why can't you give me a straight answer?! I've asked simple questions and gotten vague responses! I want to speak to someone who actually KNOWS your products!"
                ]
            },
            {
                "id": "escalating-service-complaint",
                "expected_transfer_point": 2,
                "messages": [
                    "The technician missed our appointment window yesterday. Can we reschedule?",
                    "Tomorrow doesn't work for me. I've already taken off work once for this.",
                    "No, Saturday isn't possible either. I need a weekday evening after 6pm.",
                    "This is RIDICULOUS! I've been trying to get this service for THREE WEEKS! Your scheduling system is a NIGHTMARE! I need to speak to a manager IMMEDIATELY about this situation!"
                ]
            }
        ],
        "transfer_on_third_message": [
            {
                "id": "policy-explanation-frustration",
                "expected_transfer_point": 3,
                "messages": [
                    "Can you explain your international shipping policies? I'm trying to order from overseas.",
                    "So you're saying there's no way to get free shipping internationally, even with a premium membership?",
                    "This is such a ripoff! I've spent thousands with your company and you can't even offer reasonable shipping?! I want to speak to someone in charge about this policy IMMEDIATELY!"
                ]
            },
            {
                "id": "prolonged-shipping-issue",
                "expected_transfer_point": 3,
                "messages": [
                    "My order was supposed to arrive last week, but the tracking hasn't updated. Can you check on it?",
                    "The tracking number you provided shows it's still in the warehouse, not shipped.",
                    "I called the shipping company and they said they haven't received the package from you yet.",
                    "This is COMPLETELY UNACCEPTABLE! I've been waiting for TWO WEEKS for an order you claimed was shipped! I demand to speak to a manager RIGHT NOW!"
                ]
            },
            {
                "id": "complex-return-issue",
                "expected_transfer_point": 3,
                "messages": [
                    "How do I return an item that was a gift? I don't have the original receipt.",
                    "The person who bought it used their account but shipped it to my address.",
                    "I tried using the order number they forwarded me, but the system doesn't recognize it.",
                    "I've been trying to return this for THREE WEEKS! This is the worst customer service I've ever experienced! Get me a manager NOW who can actually help me!"
                ]
            },
            {
                "id": "account-merger-complexity",
                "expected_transfer_point": 3,
                "messages": [
                    "I have two accounts with your service. Can I merge them into one?",
                    "Both accounts have subscription history and saved information I want to keep.",
                    "I tried the account settings page, but there's no option to merge accounts there.",
                    "This is beyond frustrating! I've been trying to solve this simple problem for DAYS! I need to speak to someone with actual authority who can merge these accounts IMMEDIATELY!"
                ]
            }
        ]
    }

    ```
    </div>

- `run_customer_service_agent.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting Customer Service Agent test from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script
    echo "Running Python script ..."
    python3 "$DIR/service_agent.py"

    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    </div>

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="customer_service_agent",
        srcs=["service_agent.py"],
        main="service_agent.py",
        deps=[
            "@pypi//python_dotenv",
            "@pypi//torch",
            "@pypi//transformers",
            "@pypi//scipy",
            "//src/models/claude_client:claude_client",
        ],
        data=[
            "test_conversations.json",
            "//:.env",
        ],
        imports=[
            "",
            ".",
            "../..",
        ],
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="customer_service_agent_test",
        srcs=["run_customer_service_agent.sh"],
        data=[
            ":customer_service_agent",
        ],
    )

    ```
    </div>

See the agent at work by running this file.

For local execution, use: <br>
`bazel run //src/agents/customer_service:customer_service_agent`

For remote execution, use: <br>
`bazel test //src/agents/customer_service:customer_service_agent_test`

## **3) Integrating Hugging Face Transformers**

Hugging Face's transformers library provides access to thousands of pre-trained models that can complement Claude's capabilities. Let's implement a client to work with these models within our monorepo structure.

- `transformers_client.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```python
    import torch
    from transformers import (
        AutoTokenizer,
        AutoModel,
        AutoModelForSequenceClassification,
        AutoModelForTokenClassification,
        AutoModelForQuestionAnswering,
        AutoModelForMaskedLM,
        pipeline,
    )

    class HuggingFaceClient:
        def __init__(self, task, model_name=None, device=None):
            """
            Initialize the HuggingFace client for a specific task.

            Args:
                task (str): The NLP task (e.g., 'text-classification', 'ner', 'question-answering')
                model_name (str, optional): Model name from Hugging Face Hub. If None, uses the default for the task.
                device (str, optional): Device to use ('cuda' or 'cpu'). If None, uses CUDA if available.
            """
            self.task = task
            self.device = (
                device if device else "cuda" if torch.cuda.is_available() else "cpu"
            )
            print(f"Using device: {self.device}")

            # Define default models for each task
            default_models = {
                "text-classification": "distilbert-base-uncased-finetuned-sst-2-english",
                "ner": "dslim/bert-base-NER",  # Smaller base-sized model for NER
                "question-answering": "distilbert-base-cased-distilled-squad",  # Distilled model for QA
                "fill-mask": "distilbert-base-uncased",  # Distilled model instead of full BERT
                "embeddings": "sentence-transformers/all-MiniLM-L6-v2",
            }

            # If the user has provided a model_name, use that
            if model_name is not None:
                self.model_name = model_name
            # Else, use the default model chosen for the task
            else:
                if task in default_models:
                    self.model_name = default_models[task]
                else:
                    raise ValueError(
                        f"No default model for task '{task}'. Please choose a supported task or specify a model_name."
                    )

            # Load tokenizer
            print(f"Loading tokenizer for model '{self.model_name}'")
            self.tokenizer = AutoTokenizer.from_pretrained(self.model_name)

            # Load the appropriate model based on the task
            print(f"Loading model '{self.model_name}' for task '{task}'")
            try:
                if task == "text-classification":
                    self.model = AutoModelForSequenceClassification.from_pretrained(
                        self.model_name
                    ).to(self.device)
                elif task == "ner":
                    self.model = AutoModelForTokenClassification.from_pretrained(
                        self.model_name
                    ).to(self.device)
                elif task == "question-answering":
                    self.model = AutoModelForQuestionAnswering.from_pretrained(
                        self.model_name
                    ).to(self.device)
                elif task == "fill-mask":
                    self.model = AutoModelForMaskedLM.from_pretrained(self.model_name).to(
                        self.device
                    )
                elif task == "embeddings":
                    self.model = AutoModel.from_pretrained(self.model_name).to(self.device)
                else:
                    raise ValueError(f"Unsupported task: {task}")

                # Create pipeline
                print(f"Creating pipeline for task '{task}'")
                if task == "embeddings":
                    # Transformers doesn't provide a standard pipeline for embeddings
                    # So we'll create our own get_embeddings method instead
                    self.pipeline = None
                else:
                    # Note: we're using the model directly rather than loading via model_name
                    self.pipeline = pipeline(
                        task, model=self.model, tokenizer=self.tokenizer
                    )

                print("Initialization completed successfully")

            except Exception as e:
                raise ValueError(
                    f"Failed to initialize client for task '{task}' with model '{self.model_name}': {str(e)}"
                )

        def get_embeddings(self, texts):
            """
            Generate embeddings for a list of texts.

            Args:
                texts (str or list): Input text or list of texts

            Returns:
                numpy.ndarray: The embeddings
            """
            if isinstance(texts, str):
                texts = [texts]

            encoded_input = self.tokenizer(
                texts, padding=True, truncation=True, return_tensors="pt"
            ).to(self.device)

            with torch.no_grad():
                model_output = self.model(**encoded_input)

            # For base models (AutoModel)
            if hasattr(model_output, "last_hidden_state"):
                # Use the CLS token embedding (first token)
                embeddings = model_output.last_hidden_state[:, 0, :]
            else:
                # Fallback for different output structures
                embeddings = model_output[0][:, 0, :]

            return embeddings.cpu().numpy()

        def __call__(self, *args, **kwargs):
            """
            Use the client as a function by calling the pipeline.

            Args:
                *args, **kwargs: Arguments to pass to the pipeline

            Returns:
                The output of the pipeline
            """
            if self.task == "embeddings":
                # For embeddings task, use get_embeddings method
                if len(args) == 1:
                    return self.get_embeddings(args[0])
                elif "texts" in kwargs:
                    return self.get_embeddings(kwargs["texts"])
                else:
                    raise ValueError(
                        "For embeddings task, provide text input as first arg or as 'texts' kwarg"
                    )
            elif self.pipeline is not None:
                # For other tasks, use the pipeline
                return self.pipeline(*args, **kwargs)
            else:
                raise ValueError(f"No pipeline available for task '{self.task}'")

    if __name__ == "__main__":

        print("=" * 60)
        print("Hugging Face Transformers Demo For 5 Popular NLP Tasks")
        print("=" * 60)
        print("\n=== Text Classification Demo ===")
        classification_client = HuggingFaceClient(task="text-classification")
        classification_result = classification_client("I love using Hugging Face models!")
        print(f"Classification result: {classification_result}")

        print("\n=== Named Entity Recognition Demo ===")
        ner_client = HuggingFaceClient(task="ner")
        ner_result = ner_client(
            "My name is Sarah Johnson and I work at Google in Mountain View, California."
        )
        # Print entities in a readable format
        print("Named entities:")
        current_entity = None
        entity_text = ""
        for token in ner_result:
            if current_entity != token["entity"] and entity_text:
                if current_entity != "O":  # Skip non-entities
                    print(f"  - {entity_text}: {current_entity}")
                entity_text = token["word"]
                current_entity = token["entity"]
            else:
                entity_text += token["word"].replace("##", "")
                current_entity = token["entity"]
        if current_entity != "O" and entity_text:
            print(f"  - {entity_text}: {current_entity}")

        print("\n=== Question Answering Demo ===")
        qa_client = HuggingFaceClient(task="question-answering")
        context = """
        Artificial intelligence (AI) is intelligence demonstrated by machines, as opposed to natural
        intelligence displayed by animals including humans. AI research has been defined as the field
        of study of intelligent agents, which refers to any system that perceives its environment and
        takes actions that maximize its chance of achieving its goals.
        """
        qa_result = qa_client(question="What is artificial intelligence?", context=context)
        print(f"Question: What is artificial intelligence?")
        print(f"Answer: {qa_result['answer']} (Score: {qa_result['score']:.4f})")

        print("\n=== Fill Mask Demo ===")
        mask_client = HuggingFaceClient(task="fill-mask")
        mask_text = "The [MASK] barked loudly at the intruder."
        mask_result = mask_client(mask_text)
        print(f"Original text: {mask_text}")
        print("Top 5 mask predictions:")
        for prediction in mask_result[:5]:
            print(f"  - {prediction['token_str']}: {prediction['score']:.4f}")

        print("\n=== Embeddings Demo ===")
        embedding_client = HuggingFaceClient(task="embeddings")
        texts = [
            "This is the first example sentence.",
            "Each sentence will get its own embedding vector.",
        ]
        embeddings = embedding_client.get_embeddings(texts)
        print(f"Generated embeddings with shape: {embeddings.shape}")
        # Calculate cosine similarity between the two embeddings
        from scipy.spatial.distance import cosine

        similarity = 1 - cosine(embeddings[0], embeddings[1])
        print(f"Cosine similarity between the two sentences: {similarity:.4f}")

        # Demonstrate that the client can also be called directly for embeddings
        single_embedding = embedding_client("This is a single sentence.")
        print(f"Single embedding shape: {single_embedding.shape}")

    ```
    </div>

- `run_hugging_face.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting Hugging Face / Transformers Client test from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script
    echo "Running Python script ..."
    python3 "$DIR/transformers_client.py"

    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    </div>

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="huggingface_client",
        srcs=["transformers_client.py"],
        main="transformers_client.py",
        deps=[
            "@pypi//torch",
            "@pypi//transformers",
            "@pypi//scipy",
        ],
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="hugging_face_test",
        srcs=["run_hugging_face.sh"],
        data=[
            ":huggingface_client",
        ],
    )

    ```
    </div>

See the Hugging Face models perform 5 popular NLP tasks by running this file.

For local execution, use: <br>
`bazel run //src/models/huggingface:huggingface_client`

For remote execution, use: <br>
`bazel test //src/models/huggingface:hugging_face_test`

One of the many benefits of working with Hugging Face is that Hugging Face automatically caches any downloaded model. If you run transformers_client.py again, you’ll notice significant performance improvements.

## **4) Creating an Agent That Combines Claude with Hugging Face Models**

Now, let's build an agent that leverages both Claude's capabilities and open-source HuggingFace models:

- `assistant.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```python
    import json
    import os
    import sys

    # Add root directory to Python path for absolute imports
    sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../../..")))

    # Try both import styles
    try:
        # Try absolute import first
        from src.models.claude_client.client import ClaudeClient
        from src.models.huggingface.transformers_client import HuggingFaceClient
    except ImportError:
        # Fall back to relative import if absolute fails
        from ...models.claude_client.client import ClaudeClient
        from ...models.huggingface.transformers_client import HuggingFaceClient

    class HybridAgent:
        def __init__(
            self,
            anthropic_api_key,
            embedding_model="sentence-transformers/all-MiniLM-L6-v2",
        ):
            self.claude_client = ClaudeClient(api_key=anthropic_api_key)
            self.embedding_client = HuggingFaceClient(
                task="embeddings", model_name=embedding_model
            )

        def process_query(self, query, context_documents=None):
            """
            Process a user query using a combination of Claude and local models

            Args:
                query: User's question or request
                context_documents: Optional list of documents to use as context

            Returns:
                Response to the user's query
            """
            # If we've context documents, use the embedding model to rank them
            if context_documents:
                query_embedding = self.embedding_client([query])[0]  # Updated method call
                # Get embeddings for all documents
                doc_embeddings = self.embedding_client(
                    context_documents
                )  # Updated method call

                # Calculate relevance scores (dot product)
                import numpy as np

                relevance_scores = np.dot(doc_embeddings, query_embedding)

                # Sort documents by relevance
                sorted_indices = np.argsort(relevance_scores)[::-1]
                top_docs = [context_documents[i] for i in sorted_indices[:3]]

                # Include top documents as context for Claude
                context_text = "\n\n".join(top_docs)
                augmented_query = f"Query: {query}\n\nRelevant Context:\n{context_text}"
            else:
                augmented_query = query

            # Define the response schema for Claude
            response_schema = {
                "type": "object",
                "properties": {
                    "answer": {"type": "string"},
                    "confidence": {"type": "number"},
                    "sources": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["answer", "confidence"],
            }

            # Get structured response from Claude
            system_prompt = """
            You are a helpful AI assistant that provides accurate, concise answers.
            If the context includes relevant information, use it to inform your answer.
            Always indicate your confidence in your answer on a scale from 0 to 1.
            If you use information from the context, list the relevant sources.
            """

            response = self.claude_client.get_structured_response(
                system_prompt=system_prompt,
                user_message=augmented_query,
                response_schema=response_schema,
            )

            # Add error handling for None response
            if response is None:
                return f"I'm sorry, I wasn't able to process your query: '{query}'. Please try again with a different question."

            # Format the final response
            final_response = f"{response.get('answer', 'No answer provided')}"

            if response.get("confidence") is not None:
                final_response += f"\n\nConfidence: {response.get('confidence'):.2f}"  # Format to 2 decimal places

            if response.get("sources"):
                sources = response["sources"]
                if isinstance(sources, str):
                    final_response += f"\n\nSources: {sources}"
                else:
                    final_response += f"\n\nSources: {', '.join(sources)}"

            return final_response

    if __name__ == "__main__":
        import os
        import sys
        import numpy as np
        from dotenv import load_dotenv

        load_dotenv()

        # Basic test function to organize tests
        def run_tests():
            print("Running HybridAgent tests...")

            # Test 1: Test initialization
            print("\nTest 1: Testing agent initialization...")
            try:
                api_key = os.environ.get("ANTHROPIC_API_KEY")
                if not api_key:
                    print(
                        "Warning: ANTHROPIC_API_KEY not found in environment. Using placeholder for test."
                    )
                    api_key = "dummy_key_for_testing"
                print(
                    f"ANTHROPIC_API_KEY exists: {'Yes: '+api_key if os.environ.get('ANTHROPIC_API_KEY') else 'No'}"
                )
                print(f"RUN_API_TESTS value: {os.environ.get('RUN_API_TESTS', 'Not set')}")

                # For the updated HuggingFaceClient, we need to specify task="embeddings"
                agent = HybridAgent(
                    anthropic_api_key=api_key,
                    embedding_model="sentence-transformers/all-MiniLM-L6-v2",
                )
                print("✅ Agent initialized successfully")
            except Exception as e:
                print(f"❌ Failed to initialize agent: {str(e)}")
                return False

            # Test 2: Test embedding model functionality
            print("\nTest 2: Testing embedding functionality...")
            try:
                # Create a simple test for embeddings
                test_texts = ["This is a test sentence.", "Another sentence to embed."]
                # Use the new client's __call__ method which internally calls get_embeddings for task="embeddings"
                embeddings = agent.embedding_client(test_texts)

                # Verify shape and type
                assert isinstance(
                    embeddings, np.ndarray
                ), "Embeddings should be a numpy array"
                assert embeddings.shape[0] == len(
                    test_texts
                ), f"Expected {len(test_texts)} embeddings, got {embeddings.shape[0]}"
                assert (
                    embeddings.shape[1] > 0
                ), "Embedding vectors should have non-zero dimensions"

                print(f"✅ Embedding model working correctly. Shape: {embeddings.shape}")
            except Exception as e:
                print(f"❌ Embedding test failed: {str(e)}")
                return False

            # Test 3: Mock test for Claude client
            print("\nTest 3: Testing Claude client functionality...")
            try:
                # This is a simplified test since we don't want to make actual API calls
                has_client = (
                    hasattr(agent, "claude_client") and agent.claude_client is not None
                )
                assert has_client, "Claude client not properly initialized"

                # Check if the client has the expected method
                has_method = hasattr(agent.claude_client, "get_structured_response")
                assert has_method, "Claude client missing get_structured_response method"

                print("✅ Claude client properly configured")
            except Exception as e:
                print(f"❌ Claude client test failed: {str(e)}")
                return False

            # Test 4: Test a simple query without context
            # For test 4 in your assistant.py
            print("\nTest 4: Testing simple query processing (no context)...")
            try:
                if os.environ.get("RUN_API_TESTS", "").lower() == "true" and os.environ.get(
                    "ANTHROPIC_API_KEY"
                ):
                    print("Running API test with Claude...")
                    query = "What is the capital of France?"

                    try:
                        # Try to print the API key (first few chars) to verify it's loaded correctly
                        api_key = os.environ.get("ANTHROPIC_API_KEY", "")
                        print(f"API key loaded (first 5 chars): {api_key[:5]}...")

                        # Print before making the call
                        print("Calling process_query...")

                        # Debug the structured response directly
                        print("Testing Claude client directly first...")
                        system_prompt = "You are a helpful AI assistant that provides accurate, concise answers."
                        response_schema = {
                            "type": "object",
                            "properties": {
                                "answer": {"type": "string"},
                                "confidence": {"type": "number"},
                            },
                            "required": ["answer", "confidence"],
                        }

                        structured_response = agent.claude_client.get_structured_response(
                            system_prompt=system_prompt,
                            user_message=query,
                            response_schema=response_schema,
                        )

                        print(f"Direct Claude response: {structured_response}")

                        # Now test the full process_query method
                        response = agent.process_query(query)
                        print(f"Full process_query response: {response}")

                        assert response and isinstance(
                            response, str
                        ), "Response should be a non-empty string"
                        print(
                            f"✅ Query processed successfully. Response: {response[:50]}..."
                        )
                    except Exception as e:
                        print(f"Error details during API call: {str(e)}")
                        import traceback

                        traceback.print_exc()
                        raise
                else:
                    print(
                        "⚠️ Skipping API test - set RUN_API_TESTS=true and valid ANTHROPIC_API_KEY to run"
                    )
            except Exception as e:
                print(f"❌ Query processing test failed: {str(e)}")
                return False

            # Test 5: Test query with context
            print("\nTest 5: Testing query with context...")
            try:
                # Create some test context documents
                context_docs = [
                    "Paris is the capital of France.",
                    "Tokyo is the capital of Japan.",
                    "Washington D.C. is the capital of the United States.",
                ]

                # Compare embeddings of a query with context documents
                query = "What is the capital of France?"
                query_embedding = agent.embedding_client([query])[0]  # Updated method call
                doc_embeddings = agent.embedding_client(context_docs)  # Updated method call

                # Calculate similarities (dot product)
                similarities = np.dot(doc_embeddings, query_embedding)

                # The most similar document should be the one about France
                most_similar_idx = np.argmax(similarities)
                assert (
                    most_similar_idx == 0
                ), f"Expected document 0 to be most similar, got {most_similar_idx}"

                print("✅ Context ranking working correctly")

                # Full API test with context
                if os.environ.get("RUN_API_TESTS", "").lower() == "true" and os.environ.get(
                    "ANTHROPIC_API_KEY"
                ):
                    response = agent.process_query(query, context_docs)
                    assert response and isinstance(
                        response, str
                    ), "Response should be a non-empty string"
                    print(
                        f"✅ Query with context processed successfully. Response: {response[:50]}..."
                    )
                else:
                    print(
                        "⚠️ Skipping API test with context - set RUN_API_TESTS=true to run"
                    )
            except Exception as e:
                print(f"❌ Context query test failed: {str(e)}")
                return False

            print("\nAll tests completed successfully! ✅")
            return True

        # Run the tests
        success = run_tests()
        sys.exit(0 if success else 1)

    ```
    </div>

- `run_assistant.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting Hybrid Agent test from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script
    echo "Running Python script ..."
    python3 "$DIR/assistant.py"

    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    </div>

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="hybrid_agent",
        srcs=["assistant.py"],
        main="assistant.py",
        deps=[
            "@pypi//python_dotenv",
            "//src/models/claude_client:claude_client",
            "//src/models/huggingface:huggingface_client",
        ],
        data=[
            "//:.env",  # Moved from deps to data
        ],
        imports=[
            ".",
            "../..",  # For imports starting with 'src'
        ],
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="hybrid_agent_test",
        srcs=["run_assistant.sh"],
        data=[
            ":hybrid_agent",
        ],
    )

    ```
    </div>

To see an open-source model come up with vector embeddings for context documents that are then sent to Claude to get an MCP compliant response, run `assistant.py`.

For local execution, use: <br>
`bazel run //src/agents/hybrid_assistant:hybrid_agent`

For remote execution, use: <br>
`bazel test //src/agents/hybrid_assistant:hybrid_agent_test`

## **5) Training / Fine-Tuning On NativeLink Cloud**

One of the key advantages of our setup is the ability to leverage powerful cloud resources through NativeLink's remote execution capabilities. This demo uses CPUs for fine-tuning. Let's create a script that demonstrates how to run training jobs on remote, virtual CPUs:

- `train_model.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```python
    import torch
    import numpy as np
    import os
    import time

    from transformers import (
        AutoModelForSequenceClassification,
        AutoTokenizer,
        Trainer,
        TrainingArguments,
    )
    from datasets import load_dataset

    def print_model_performance(model, trainer, tokenizer, test_dataset):
        """
        Function to print model performance metrics

        Note:
        This inferencing is set up for output shape returned by prajjwal1/bert-tiny
        since that's the default model trained by train_classifier function.
        If you use a different model, you may need to adjust the output shape
        """
        print("\nEvaluating model accuracy on test set...")
        # Load the test set
        train_size = len(test_dataset)
        print(f"Test dataset size: {len(test_dataset)}")

        # Tokenize dataset
        def tokenize_function(examples):
            return tokenizer(
                examples["sentence"],
                padding="max_length",
                max_length=128,  # Set an explicit max length
                truncation=True,
                return_tensors=None,  # Important: don't return tensors yet
            )

        # Tokenize the test dataset
        tokenized_test = test_dataset.map(tokenize_function, batched=True)

        # Run prediction on the test set
        predictions = trainer.predict(tokenized_test)

        # Calculate accuracy using NumPy
        logits = predictions.predictions
        predicted_classes = np.argmax(logits, axis=1)
        labels = predictions.label_ids

        accuracy = np.mean(predicted_classes == labels)

        print(f"\nTest accuracy: {accuracy:.4f} ({accuracy*100:.2f}%)")
        print(f"Test loss: {predictions.metrics['test_loss']:.4f}")

        # Calculate basic metrics without sklearn
        true_positives = np.sum((predicted_classes == 1) & (labels == 1))
        true_negatives = np.sum((predicted_classes == 0) & (labels == 0))
        false_positives = np.sum((predicted_classes == 1) & (labels == 0))
        false_negatives = np.sum((predicted_classes == 0) & (labels == 1))

        print(f"\nConfusion Matrix (calculated with NumPy):")
        print(f"True Positives: {true_positives}")
        print(f"True Negatives: {true_negatives}")
        print(f"False Positives: {false_positives}")
        print(f"False Negatives: {false_negatives}")

        # Precision and recall
        precision = (
            true_positives / (true_positives + false_positives)
            if (true_positives + false_positives) > 0
            else 0
        )
        recall = (
            true_positives / (true_positives + false_negatives)
            if (true_positives + false_negatives) > 0
            else 0
        )

        print(f"Precision: {precision:.4f}")
        print(f"Recall: {recall:.4f}")
        return

    def train_classifier(
        model_name="prajjwal1/bert-tiny",
        dataset_name="glue",
        dataset_config="sst2",
        output_dir=None,
        batch_size=16,
        learning_rate=5e-5,
        epochs=2,
    ):
        if output_dir is None:
            output_dir = os.path.join(
                "~/.cache/huggingface/hub",
                model_name.replace("/", "_"),
                # Default Hugging Face cache directory
            )

        # Expand user path if needed
        if output_dir and output_dir.startswith("~"):
            output_dir = os.path.expanduser(output_dir)

        # model_dir = os.path.join(output_dir, model_name.replace("/", "_"))
        os.makedirs(output_dir, exist_ok=True)
        print(f"Created model directory at: {output_dir}")

        # Create checkpoints subdirectory
        checkpoints_dir = os.path.join(output_dir, "checkpoints")
        os.makedirs(checkpoints_dir, exist_ok=True)
        print(f"Created checkpoints directory at: {checkpoints_dir}")

        # Load dataset
        dataset = load_dataset(dataset_name, dataset_config)
        print(f"Finished loading dataset: {dataset_name}!")

        total_training_samples = len(dataset["train"])
        print(f"Total training samples: {total_training_samples}")
        # Reserve the last 1000 samples for testing
        num_testing_samples = 1000
        test_dataset = dataset["train"].select(
            range(total_training_samples - num_testing_samples, total_training_samples)
        )
        print("Using half of dataset for faster training!")
        dataset["train"] = dataset["train"].select(range(total_training_samples // 2))

        # Load tokenizer and model
        tokenizer = AutoTokenizer.from_pretrained(model_name)
        print("Finished loading tokenizer!")
        model = AutoModelForSequenceClassification.from_pretrained(model_name, num_labels=2)
        print("Finished loading model!")

        # Tokenize dataset
        def tokenize_function(examples):
            return tokenizer(
                examples["sentence"],
                padding="max_length",
                max_length=128,  # Set an explicit max length
                truncation=True,
                return_tensors=None,  # Important: don't return tensors yet
            )

        tokenized_datasets = dataset.map(tokenize_function, batched=True)
        print("Finished tokenizing dataset!")

        # Define training arguments
        training_args = TrainingArguments(
            output_dir=checkpoints_dir,
            per_device_train_batch_size=batch_size,
            per_device_eval_batch_size=batch_size,
            learning_rate=learning_rate,
            num_train_epochs=epochs,
            weight_decay=0.01,
            evaluation_strategy="epoch",
            save_strategy="epoch",
            load_best_model_at_end=True,
        )
        print("Finished setting up training arguments!")

        # Define trainer
        trainer = Trainer(
            model=model,
            args=training_args,
            train_dataset=tokenized_datasets["train"],
            eval_dataset=tokenized_datasets["validation"],
        )
        print("Finished setting up trainer instance!")

        print("=" * 80)
        print("Model Performance Before Fine Tuning")
        print("=" * 80)
        print_model_performance(model, trainer, tokenizer, test_dataset)

        # To monitor time taken for training
        start_time = time.time()

        print("=" * 80)
        print("Training The Model")
        print("=" * 80)
        # Train model
        trainer.train()
        print("\nFinished training the model!")

        # End timing
        end_time = time.time()
        training_time = end_time - start_time
        print(f"Model trained in {training_time:.2f} seconds")

        # After setting up the trainer
        print("=" * 80)
        print("Model architecture")
        print("=" * 80)
        print(f"Model type: {trainer.model.__class__.__name__}")

        # Get parameter counts
        total_params = sum(p.numel() for p in trainer.model.parameters())
        trainable_params = sum(
            p.numel() for p in trainer.model.parameters() if p.requires_grad
        )
        print(f"Total parameters: {total_params:,}")
        print(f"Trainable parameters: {trainable_params:,}\n")

        # After training and before saving, run an inference test
        test_text = "This movie was really good!"
        print("=" * 80)
        print(f"A sample test inference before saving")
        print("=" * 80)
        print(f"Test text: '{test_text}'")

        # Get the device the model is currently on
        # retrieves the device of the first parameter tensor in your model.
        # This gives you the exact device where your model currently resides.
        # This makes sure that model fine-tuned remotely is accessible locally if needs be.
        device = next(model.parameters()).device

        # Ensure inputs are on the same device
        inputs = tokenizer(test_text, return_tensors="pt")
        inputs = {
            k: v.to(device) for k, v in inputs.items()
        }  # moves all your input tensors to that same device.
        with torch.no_grad():
            outputs = model(**inputs)

        logits = outputs.logits
        predicted_class = torch.argmax(logits, dim=1).item()

        print(f"Predicted class before saving: {predicted_class}")

        # Save model
        model.save_pretrained(output_dir)
        tokenizer.save_pretrained(output_dir)
        print("Finished saving model related files!\n")

        print("=" * 80)
        print("Model Performance After Fine Tuning On NativeLink Cloud For 3 Minutes")
        print("=" * 80)
        print_model_performance(model, trainer, tokenizer, test_dataset)

        print("*" * 80)
        print("-" * 80)

        return output_dir

    if __name__ == "__main__":

        # Detect if CUDA is available
        device = "cuda" if torch.cuda.is_available() else "cpu"
        print(f"Using device: {device}")

        if device == "cuda":
            # Print NVIDIA GPU information
            print(f"NVIDIA GPU: {torch.cuda.get_device_name(0)}")
            print(f"CUDA Version: {torch.version.cuda}")

        # Train the model
        # cache_dir = "~/.cache/huggingface/hub"
        output_path = train_classifier()
        print(f"Model saved to {output_path}")

        # Load the saved model to verify
        print("Loading saved model to verify...")
        loaded_model = AutoModelForSequenceClassification.from_pretrained(output_path)
        loaded_tokenizer = AutoTokenizer.from_pretrained(output_path)

        # Count parameters
        total_params = sum(p.numel() for p in loaded_model.parameters())
        trainable_params = sum(
            p.numel() for p in loaded_model.parameters() if p.requires_grad
        )

        print(f"Verification - Loaded model parameters: {total_params:,}")
        print(f"Verification - Loaded trainable parameters: {trainable_params:,}")

        # Optional: Try a simple inference to confirm everything works
        test_text = "This movie was really good!"
        inputs = loaded_tokenizer(test_text, return_tensors="pt")
        with torch.no_grad():
            outputs = loaded_model(**inputs)

        logits = outputs.logits
        predicted_class = torch.argmax(logits, dim=1).item()
        print(f"Test inference on '{test_text}' - Predicted class: {predicted_class}")

    ```
    </div>

- `run_training.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting ML Training test from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script
    echo "Running Python script ..."
    python3 "$DIR/train_model.py"

    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    </div>

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="train_model",
        srcs=["train_model.py"],
        deps=[
            "@pypi//torch",
            "@pypi//transformers",
            "@pypi//datasets",
            "@pypi//accelerate",
            "@pypi//psutil",
        ],
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="training_test",
        srcs=["run_training.sh"],
        data=[
            ":train_model",
        ],
        size="large",
        # size="enormous",
    )

    ```
    </div>

To observe the before-and-after effects of fine-tuning the lightweight model `"prajjwal1/bert-tiny"` for just 3 minutes on our cloud (targeting text sentiment analysis), run `train_model.py`

For local execution, use: <br>
`bazel run //src/training:train_model`

For remote execution, use: <br>
`bazel test //src/training:training_test`

### The NativeLink Difference:

To demonstrate NativeLink’s efficacy, consistency, and reliability, we ran the same fine-tuning job on the CPU of a M1 Pro MacBook Pro, the free version of Google Colab on CPU, and [NativeLink](https://github.com/TraceMachina/nativelink),which is free and open-source. We executed the fine-tuning task 5 times and this is what we observed:

1\. The Mac: the quickest run took 18 minutes while the slowest/longest took 20 minutes

2\. Free version of Google Colab: the quickest run took 10 minutes while the slowest/longest took 20 minutes. The execution time was widely varied. We suspect varying traffic on Google’s servers and how Colab allocates its compute resources played a part in this variability.

3\. Free NativeLink: the quickest run took 2 minutes while the slowest/longest took 3 minutes. NativeLink Cloud provided the quickest execution times by far.

<img src="https://nativelink-nativelink-nativelink.notion.site/image/attachment%3A44fa2a4b-432f-41fc-8156-e67c1606e726%3ATrace_Machina.png?table=block&id=1d6dbbd6-a6bd-80ad-bd8f-fd09fcbe1419&spaceId=448d4308-e035-401c-9af7-c04687aca8e5" width="1000" alt="Model Fine-Tuning Times">

## **6) Caching Model Weights**

One of the big advantages of our setup is the ability to cache model weights and artifacts, which can significantly speed up development time. Let's create a small utility that helps manage model downloads/caching and handles Hugging Face’s built-in automatic caching differently:

- `cache_manager.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```python
    import os
    import sys
    import hashlib
    from pathlib import Path
    from transformers import AutoModelForSequenceClassification, AutoTokenizer, AutoModel

    sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../..")))

    try:
        # Try the local import path first
        from src.training.train_model import train_classifier
    except ModuleNotFoundError:
        # Fall back to the remote import path
        from ..training.train_model import train_classifier

    class ModelCacheManager:
        def __init__(self, cache_dir=None):
            self.cache_dir = cache_dir or os.path.expanduser("~/.cache/huggingface/hub")
            print("-" * 80)
            print("Cache directory Set To:", self.cache_dir)
            print("-" * 80)
            os.makedirs(self.cache_dir, exist_ok=True)

        def get_cache_path(self, model_name, revision="main"):
            """Get the path where a model should be cached"""
            # Create a unique identifier for this model version
            model_id = f"{model_name}_{revision}"
            cache_hash = hashlib.sha256(model_id.encode()).hexdigest()

            return Path(self.cache_dir) / cache_hash

        def is_cached_manually(self, model_name, revision="main"):
            """
            Check if a model was cached manually (so not Hugging Face)
            By checking the existence of cached files saved under the cache directory
            """

            # Determine the cache path for this model
            model_cache_path = self.get_cache_path(model_name, revision)
            return model_cache_path.exists()

        def is_cached_by_hugging_face(self, model_name, revision="main"):
            """
            Check if a model is already cached automatically by Hugging Face
            By checking the existence of cached files saved under Hugging Face's
            default naming conventions
            """

            # Determine Hugging Face's cache path
            hf_cache_dir = (
                self.cache_dir
                if "huggingface/hub" in self.cache_dir
                else "~/.cache/huggingface/hub"
            )
            hf_cache_dir = os.path.expanduser(hf_cache_dir)  # Expand ~ to home directory
            # Convert model_name format: "org/model" → "models--org--model"
            hf_model_path = "models--" + model_name.replace("/", "--")
            hugging_face_cache_path = Path(hf_cache_dir) / hf_model_path
            # Simple directory existence check
            return hugging_face_cache_path.exists()

        # We don't have a save_model as the train_model function will save the model

        def load_model(self, model_name, revision="main"):
            model_path = self.get_cache_path(model_name, revision)
            if model_path.exists():
                print(f"Loading model from {model_path}")
                # Load the model from the cache
                model = AutoModelForSequenceClassification.from_pretrained(model_path)
                tokenizer = AutoTokenizer.from_pretrained(model_path)
                return model, tokenizer
            else:
                print(f"Model not found in cache at {model_path}")
                return None

    if __name__ == "__main__":
        # Packages needed just for demoing and not for the cache manager itself
        import time
        import torch
        from transformers import AutoTokenizer, AutoModel

        # Choose a very lightweight model for demonstration
        model_name = "prajjwal1/bert-tiny"  # Only about 17MB, 2-layer BERT
        revision = "main"

        # Initialize the cache manager
        cache_manager = ModelCacheManager()

        print("=" * 100)
        print(
            "FIRST RUN WITH A HUGGING FACE MODEL - MODEL SHOULD NOT BE IN OUR CACHE UNLESS USED PREVIOUSLY IN YOUR ENVIRONMENT"
        )
        print("=" * 100)

        print(
            "Is model cached by Hugging Face: ",
            cache_manager.is_cached_by_hugging_face(model_name, revision),
        )
        start_time = time.time()
        # First run - should download the model
        tokenize1 = AutoTokenizer.from_pretrained(model_name, revision=revision)
        model1 = AutoModel.from_pretrained(model_name, revision=revision)
        end_time = time.time()
        print(f"Model loading time: {end_time - start_time:.2f} seconds")

        print("\n" + "=" * 80)
        print("SECOND RUN WITH A HUGGING FACE MODEL - MODEL SHOULD BE IN OUR CACHE")
        print("=" * 80)

        print(
            "Is model cached by Hugging Face: ",
            cache_manager.is_cached_by_hugging_face(model_name, revision),
        )

        start_time = time.time()
        # Second run - should use cached model
        tokenizer2 = AutoTokenizer.from_pretrained(model_name, revision=revision)
        model2 = AutoModel.from_pretrained(model_name, revision=revision)
        end_time = time.time()
        print(f"Model loading time: {end_time - start_time:.2f} seconds\n")

        # Check if models are the same
        print("=" * 80)
        print("Comparing model parameters...")
        print("=" * 80)
        for (name1, p1), (name2, p2) in zip(
            model1.named_parameters(), model2.named_parameters()
        ):
            if not torch.allclose(p1, p2):
                print(f"Parameters differ: {name1}")
                break
        else:
            print(
                "All parameters match between runs - successfully using the same model!\n"
            )

        print("*" * 80)
        print("FIRST RUN WITH A MANUALLY TRAINED MODEL")
        print("*" * 80)
        model_name = "prajjwal1/bert-tiny"
        revision = "main"
        model_path = cache_manager.get_cache_path(model_name, revision)

        # First run - should train the model
        is_model_available = cache_manager.is_cached_manually(model_name, revision)
        print("Is a manually trained model cached: ", is_model_available)
        start_time = time.time()
        if not is_model_available:
            print("Training a new model...")
            # Train the model and save it to the cache
            output_path = train_classifier(
                model_name=model_name,
                output_dir=str(model_path),
            )
            # Now load the model
            model_1, tokenizer_1 = cache_manager.load_model(model_name, revision)
        else:
            print("Loading model from cache...")
            # Load the model from the cache
            model_1, tokenizer_1 = cache_manager.load_model(model_name, revision)

        end_time = time.time()
        print(f"Model loading time: {end_time - start_time:.2f} seconds")

        print("\n" + "*" * 80)
        print("SECOND RUN WITH A MANUALLY TRAINED MODEL - MODEL SHOULD BE IN OUR CACHE")
        print("*" * 80)

        # Second run - should use cached model
        is_model_available = cache_manager.is_cached_manually(model_name, revision)
        print("Is a manually trained model cached: ", is_model_available)
        start_time = time.time()
        if not is_model_available:
            print("Training a new model...")
            # Train the model and save it to the cache
            output_path = train_classifier(
                model_name=model_name,
                output_dir=str(model_path),
            )
            # Now load the model
            model_2, tokenizer_2 = cache_manager.load_model(model_name, revision)
        else:
            print("Loading model from cache...")
            # Load the model from the cache
            model_2, tokenizer_2 = cache_manager.load_model(model_name, revision)

        end_time = time.time()
        print(f"Model loading time: {end_time - start_time:.2f} seconds\n")

        # Check if models are the same
        print("*" * 80)
        print("Comparing model parameters...")
        print("*" * 80)
        for (name1, p1), (name2, p2) in zip(
            model_1.named_parameters(), model_2.named_parameters()
        ):
            if not torch.allclose(p1, p2):
                print(f"Parameters differ: {name1}")
                break
        else:
            print("All parameters match between runs - successfully using the same model!")

    ```
    </div>

- `run_cache_manager.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting Cache Manager trial from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script
    echo "Running Python script ..."
    python3 "$DIR/cache_manager.py"

    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    </div>

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="cache_manager",
        srcs=["cache_manager.py"],
        deps=[
            "@pypi//torch",
            "@pypi//transformers",
            "@pypi//accelerate",
            "@pypi//psutil",
            "//src/training:train_model",  # to access train_classifier
        ],
        imports=[".."],  # Add this line to allow imports relative to the workspace root
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="cache_manager_test",
        srcs=["run_cache_manager.sh"],
        data=[":cache_manager"],
    )

    ```
    </div>

Run `cache_manager.py` to see the performance improvements of caching both a Hugging Face model and a manually fine-tuned model.

For local execution, use: <br>
`bazel run //src/cache_manager:cache_manager`

For remote execution, use: <br>
`bazel test //src/cache_manager:cache_manager_test`

### The NativeLink Difference:

To show how the Cache Manager takes advantage of NativeLink Cloud’s infrastructure, we ran the Cache Manager on a M1 Pro MacBook, free version of Google Colab, and free NativeLink, and we report the time it took to load cached Hugging Face models of varying size. We experimented with:

1\. prajjwal1/bert-tiny - 4.4M parameters

2\. distilbert-base-uncased - 66M parameters

3\. HuggingFaceTB/SmolLM-1.7B-Instruct - 1.7B parameters

<img src="https://img.notionusercontent.com/s3/prod-files-secure%2F448d4308-e035-401c-9af7-c04687aca8e5%2Febf4b7b0-4999-4873-9697-aeba17740278%2F12029e53-64b7-487b-a82b-426b1f600d63.png/size/w=2000?exp=1746313414&sig=dL75V4VJVvySbVc_N5JsuQ9CQuE71wJM-EcnHvnVedQ&id=1d6dbbd6-a6bd-80bb-9e70-fb821d0bc436" width="1000" alt="Time Needed to load Cached Hugging Face Model Using Cache Manager">

Observations:

1\. All three environments were fastest when loading the 66M parameter model instead of loading the smallest 4.4M parameter model

2\. Free version of Google Colab doesn't provide enough RAM to load the 1.7B parameter model. We tried 7 times but the session crashed every time.

3\. NativeLink scales very well with bigger data bandwidths. It took less than 1 second to load a model that the free version of Colab just couldn’t.

## **7) Putting It All Together in a Command-Line Tool**

Let’s come up with a command line tool that shows how the different components work together.

- `complete_agent.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```python
    #!/usr/bin/env python3
    import os
    import sys
    import argparse
    from dotenv import load_dotenv

    # Add root directory to Python path for absolute imports
    sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../..")))

    try:
        # Try absolute import first
        from src.agents.hybrid_assistant.assistant import HybridAgent
        from src.cache_manager.cache_manager import ModelCacheManager
    except ImportError:
        # Fall back to relative import if absolute fails
        from ..agents.hybrid_assistant.assistant import HybridAgent
        from ..cache_manager.cache_manager import ModelCacheManager

    def run_examples(agent, cache_manager):
        """Run example queries with the HybridAgent."""
        print("\n✨ Running HybridAgent Examples ✨\n")

        # Example 1: Basic query
        example_query = "What is Model Context Protocol?"
        print(f"🔍 Example 1: Basic Query")
        print(f'🔎 Query: "{example_query}"')
        print("=" * 50)
        response = agent.process_query(example_query)
        print("\n" + "=" * 50)
        print("🤖 Agent Response:")
        print("=" * 50)
        print(response)
        print("\n" + "=" * 70)

        # Example 2: Context-enhanced query
        example_query = "How does Hugging Face integration work with AI systems?"
        print(f"\n\n📚 Example 2: Context-Enhanced Query")
        print(f'🔎 Query: "{example_query}"')

        # Load sample context
        script_dir = os.path.dirname(os.path.abspath(__file__))
        context_file = os.path.join(script_dir, "sample_context.txt")

        if os.path.exists(context_file):
            print(f"📝 Loading context from sample_context.txt")
            with open(context_file, "r") as f:
                context_documents = f.read().split("\n\n")
            print(f"📚 Loaded {len(context_documents)} context documents.")

            print("=" * 50)
            response = agent.process_query(example_query, context_documents)
            print("\n" + "=" * 50)
            print("🤖 Agent Response:")
            print("=" * 50)
            print(response)
        else:
            print(f"❌ Sample context file not found at: {context_file}")
            print("Running without context instead...")
            print("=" * 50)
            response = agent.process_query(example_query)
            print("\n" + "=" * 50)
            print("🤖 Agent Response:")
            print("=" * 50)
            print(response)

        print("\n🚀 Examples completed! Use --help to see all available options. 🚀\n")

    # Create a custom help formatter to include examples
    class CustomHelpFormatter(argparse.HelpFormatter):
        def __init__(self, prog):
            super().__init__(prog, max_help_position=40, width=100)

        def _format_usage(self, usage, actions, groups, prefix):
            usage = super()._format_usage(usage, actions, groups, prefix)
            example_text = """
    ✨ HybridAgent Example Usage ✨

    🔍 Basic Query Example:
    bazel run //src/demo:complete_agent -- --query "What is Model Context Protocol?"

    📚 Context-Enhanced Query Example:
    bazel run //src/demo:complete_agent -- --query "How does Hugging Face integration work?" --context_file src/demo/sample_context.txt

    🧪 Run Example Queries:
    bazel run //src/demo:complete_agent -- --examples

    🚀 Happy querying! 🚀\n
    """
            return f"{usage}\n{example_text}"

    def main():

        # Load environment variables from .env file
        load_dotenv()

        parser = argparse.ArgumentParser(
            description="Run the hybrid AI agent that combines Claude and local models",
            formatter_class=CustomHelpFormatter,
        )

        # Create mutually exclusive group for the modes
        mode_group = parser.add_mutually_exclusive_group(required=True)
        mode_group.add_argument(
            "--examples",
            action="store_true",
            help="Run example queries to demonstrate the agent's capabilities",
        )
        mode_group.add_argument(
            "--query",
            type=str,
            metavar="QUERY",
            help="The user query/question to process via Claude & MCP",
        )

        # Create a separate argument group for query-related options
        query_options = parser.add_argument_group(
            "query options", "These options can only be used with --query"
        )
        query_options.add_argument(
            "--context_file", type=str, help="Optional file containing context documents"
        )
        query_options.add_argument(
            "--embedding_model",
            type=str,
            default="sentence-transformers/all-MiniLM-L6-v2",
            help="Embedding model to use for context ranking",
        )
        query_options.add_argument(
            "--run_api",
            action="store_true",
            help="Force API calls even if RUN_API_TESTS is not set to 'true'",
        )

        args = parser.parse_args()

        # Validate that NO additional flags are used with examples
        if args.examples:
            if (
                args.context_file is not None
                or args.embedding_model != "sentence-transformers/all-MiniLM-L6-v2"
                or args.run_api
            ):
                parser.error("When using --examples, no other flags can be specified")

        # If examples flag is set, run the example queries
        if args.examples:
            # Check if API key is available and if we should run API tests
            api_key = os.environ.get("ANTHROPIC_API_KEY")
            run_api_tests = (
                os.environ.get("RUN_API_TESTS", "False").lower() == "true" or args.run_api
            )

            if not run_api_tests:
                print(
                    "⚠️  Warning: RUN_API_TESTS environment variable is not set to 'true'."
                )
                print("❌ Stopping the test for example cases.")
                print(
                    "✅ To enable API calls for the example cases, set RUN_API_TESTS=true in your .env file"
                )
                sys.exit(1)

            if not api_key:
                print("❌ Error: ANTHROPIC_API_KEY not found in environment variables.")
                print("👉 Please set it in your .env file or as an environment variable.")
                sys.exit(1)

            # Initialize the cache manager for examples
            cache_manager = ModelCacheManager()

            # Initialize the agent for examples
            print("🤖 Initializing the HybridAgent for examples...")
            agent = HybridAgent(
                anthropic_api_key=api_key,
                embedding_model="sentence-transformers/all-MiniLM-L6-v2",
            )

            # Run the example queries
            run_examples(agent, cache_manager)
            return 0

        # Check if API key is available and if we should run API tests
        api_key = os.environ.get("ANTHROPIC_API_KEY")
        run_api_tests = (
            os.environ.get("RUN_API_TESTS", "False").lower() == "true" or args.run_api
        )

        if not run_api_tests:
            print("⚠️  Warning: RUN_API_TESTS environment variable is not set to 'true'.")
            print("❌ API calls will be disabled unless you use the --run_api flag.")
            print("✅ To enable API calls, either:")
            print("  1. Set RUN_API_TESTS=true in your .env file, or")
            print("  2. Use the --run_api command-line flag")
            sys.exit(1)

        if not api_key:
            print("❌ Error: ANTHROPIC_API_KEY not found in environment variables.")
            print("👉 Please set it in your .env file or as an environment variable.")
            sys.exit(1)

        # Initialize the cache manager
        cache_manager = ModelCacheManager()

        # Check if embedding model is cached
        print(f"🔍 Checking if embedding model {args.embedding_model} is cached...")
        is_cached = cache_manager.is_cached_by_hugging_face(args.embedding_model)
        print(f"💾 Embedding model is cached: {is_cached}")

        # Initialize the agent
        print("🤖 Initializing the HybridAgent...")
        agent = HybridAgent(anthropic_api_key=api_key, embedding_model=args.embedding_model)

        # Load context documents if provided
        context_documents = None
        if args.context_file:
            print(f"📚 Loading context from {args.context_file}...")
            if not os.path.exists(args.context_file):
                print(f"❌ Error: Context file {args.context_file} not found.")
                sys.exit(1)

            with open(args.context_file, "r") as f:
                context_documents = f.read().split("\n\n")
            print(f"📝 Loaded {len(context_documents)} context documents.")

        # Process the query
        print(f"🔎 Processing query: {args.query}")
        response = agent.process_query(args.query, context_documents)

        print("\n" + "=" * 50)
        print("🤖 Agent Response:")
        print("=" * 50)
        print(response)

        return 0

    if __name__ == "__main__":
        sys.exit(main())

    ```
    </div>

- `sample_context.txt -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```
    Anthropic is an AI safety company working to ensure AI systems are aligned with human values.

    Claude is a family of AI assistants made by Anthropic that can have natural conversations and perform complex tasks.

    The Claude 3 model family includes variants such as Claude 3 Opus, Claude 3 Sonnet, and Claude 3 Haiku, each with different capabilities and performance characteristics.

    Hugging Face is an AI community and platform that provides access to thousands of pre-trained models, datasets, and tools for natural language processing and machine learning.

    NativeLink is a remote execution and caching system that can be hosted in your cloud environment, helping to optimize AI development workflows.

    Enterprise-grade AI systems often require sophisticated infrastructure to handle development velocity, infrastructure costs, and engineering complexity.

    Model Context Protocol is a system for structured communication with AI models, making it easier to build reliable AI applications.

    Remote caching for model weights can significantly speed up development time by avoiding redundant downloads of large model files.

    ```
    </div>

- `run_complete_agent.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting Complete Demo test from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script with all arguments passed to the shell script
    echo "Running Python script with arguments: $@"
    python3 "$DIR/complete_agent.py" "$@"
    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    </div>

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="complete_agent",
        srcs=["complete_agent.py"],
        main="complete_agent.py",
        deps=[
            "@pypi//python_dotenv",
            "//src/cache_manager:cache_manager",
            "//src/agents/hybrid_assistant:hybrid_agent",
            "//src/models/claude_client:claude_client",
            "//src/models/huggingface:huggingface_client",
        ],
        data=["//:.env", "sample_context.txt"],
        imports=[
            ".",
            "../..",
        ],
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="complete_agent_test",
        srcs=["run_complete_agent.sh"],
        data=[
            ":complete_agent",
        ],
    )

    ```
    </div>

**Command line flags/options:**
<br>
-h, --help ----------\> show this help message and exit
<br>
--examples ----------\> Run example queries to demonstrate the agent's capabilities
<br>
--query QUERY ----------\> The user query/question to process via Claude & MCP

You can either use just the -examples flag or the -query flag with any of the following options:
<br>
--context_file CONTEXT_FILE Optional file containing context documents
<br>
--embedding_model EMBEDDING_MODEL Embedding model to use for context ranking
<br>
--run_api Force API calls even if RUN_API_TESTS isn't set to 'true'

Complete Agent Example Usage When **Locally Executed**:

1\. Run Example Queries:<br>
`bazel run //src/demo:complete_agent -- --examples`

2\. Basic Query Example:<br>
`bazel run //src/demo:complete_agent -- --query "What is Model Context Protocol?"`

3\. Context-Enhanced Query Example:<br>
`bazel run //src/demo:complete_agent -- --query "How does Hugging Face integration work?" --context_file src/demo/sample_context.txt`

Complete Agent Example Usage When **Remotely Executed**:

1\. Run Example Queries:<br>
`bazel test //src/demo:complete_agent_test --test_arg="--examples"`

2\. Basic Query Example:<br>
`bazel test //src/demo:complete_agent_test --test_arg="--query" --test_arg="What is Model Context Protocol?”`

3\. Context-Enhanced Query Example With Forced API Usage:<br>
`bazel test //src/demo:complete_agent_test --test_arg="--query" --test_arg="What is Model Context Protocol?" --test_arg="--context_file" --test_arg="src/demo/sample_context.txt" --test_arg="--run_api"`

## **8) Future Task - Improving Customer Service Agent**

We can use Hugging Face models to make the Customer Service AI Agent more cost-effective. Claude variants consistently provide good judgement when it comes to seeking human intervention. But using them to also come up with responses to show the customer doesn’t scale well monetarily. A more efficient way is to keep using Claude for the reasoning part and switch to using a Hugging Face model to generate responses to the customer. Claude would only get the customer’s responses lest discrepancies in responses generated by the Hugging Face model might tar Claude’s consistency. Moreover, we could use a light-weight Hugging Face model that's fine-tuned for conversations to generate customer-facing responses. Models like SmolLM or Google’s gemma-2-2b-it for this task can potentially happen on the customer’s device as well.

- `hybrid_service_agent.py -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```python
    import os
    import sys
    import json
    import re
    import torch
    from typing import List, Dict, Tuple, Optional, Any
    from enum import Enum
    from dataclasses import dataclass

    # Add root directory to Python path for absolute imports
    sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../../..")))

    # Try both import styles for compatibility
    try:
        # Try absolute import first
        from src.models.claude_client.client import ClaudeClient
    except ImportError:
        # Fall back to relative import if absolute fails
        from ...models.claude_client.client import ClaudeClient

    from transformers import AutoTokenizer, AutoModelForCausalLM

    @dataclass
    class ConversationMessage:
        """Data class representing a single message in a conversation"""

        sender: str  # "customer" or "agent"
        content: str
        timestamp: str = ""
        metadata: Dict[str, Any] = None

        def __post_init__(self):
            """Initialize default values for optional fields"""
            import datetime

            if not self.timestamp:
                self.timestamp = datetime.datetime.now().isoformat()
            if self.metadata is None:
                self.metadata = {}

    @dataclass
    class Conversation:
        """Data class representing a customer service conversation"""

        conversation_id: str
        history: List[ConversationMessage] = None
        metadata: Dict[str, Any] = None

        def __post_init__(self):
            """Initialize default values for optional fields"""
            if self.history is None:
                self.history = []
            if self.metadata is None:
                self.metadata = {}

        def add_message(
            self, sender: str, content: str, metadata: Dict[str, Any] = None
        ) -> None:
            """Add a message to the conversation history"""
            if metadata is None:
                metadata = {}

            message = ConversationMessage(sender=sender, content=content, metadata=metadata)

            self.history.append(message)

        def get_conversation_text(self) -> str:
            """Get the full conversation as a formatted text string"""
            text = ""
            for message in self.history:
                prefix = "Customer: " if message.sender == "customer" else "Agent: "
                text += f"{prefix}{message.content}\n\n"
            return text

    class CustomerServiceAgent:
        """
        Customer service agent that uses a light-weight Hugging Face model
        for responses and Claude MCP for transfer decisions
        """

        def __init__(
            self,
            anthropic_api_key: str,
            model_name: str = "HuggingFaceTB/SmolLM-1.7B-Instruct",
            device: str = None,
        ):
            """
            Initialize the customer service agent

            Args:
                anthropic_api_key: API key for Anthropic's Claude
                model_name: HuggingFace model for conversation
                device: Device to use for inference ("cuda", "cpu", etc.)
            """
            self.anthropic_api_key = anthropic_api_key
            self.model_name = model_name
            self.device = (
                device if device else ("cuda" if torch.cuda.is_available() else "cpu")
            )

            # Initialize Claude client for MCP
            self.claude_client = ClaudeClient(api_key=anthropic_api_key)

            # Initialize a light-weight Hugging Face for responses
            print(f"Loading Hugging Face model {model_name} on {self.device}...")
            self.tokenizer = AutoTokenizer.from_pretrained(model_name)
            if self.tokenizer.pad_token is None:
                self.tokenizer.pad_token = self.tokenizer.eos_token
                print("Set pad_token to eos_token as it was not defined")
            self.model = AutoModelForCausalLM.from_pretrained(
                model_name,
                torch_dtype=torch.bfloat16,
            ).to(self.device)
            print(f"Hugging Face model loaded successfully!")

        def evaluate_transfer_need(
            self, conversation: Conversation, verbose=False
        ) -> Dict[str, Any]:
            """
            Use Claude MCP to evaluate if a conversation needs human transfer

            Args:
                conversation: The customer conversation to evaluate

            Returns:
                Structured response with transfer decision and reasoning
            """
            # Define the response schema for Claude MCP
            response_schema = {
                "type": "object",
                "properties": {
                    "transfer_needed": {
                        "type": "boolean",
                        "description": "Whether this conversation should be transferred to a human agent",
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Detailed reasoning for the transfer decision",
                    },
                    "detected_emotion": {
                        "type": "object",
                        "properties": {
                            "primary_emotion": {
                                "type": "string",
                                "enum": [
                                    "anger",
                                    "anxiety",
                                    "frustration",
                                    "worry",
                                    "confusion",
                                    "neutral",
                                    "satisfaction",
                                    "urgency",
                                ],
                                "description": "Primary emotion detected in the customer's message",
                            },
                            "intensity": {
                                "type": "string",
                                "enum": ["low", "medium", "high"],
                                "description": "Intensity of the detected emotion",
                            },
                            "confidence": {
                                "type": "number",
                                "minimum": 0,
                                "maximum": 1,
                                "description": "Confidence in the emotion detection (0-1)",
                            },
                        },
                        "required": ["primary_emotion", "intensity", "confidence"],
                        "description": "Analysis of the customer's emotional state",
                    },
                },
                "required": ["transfer_needed", "reasoning", "detected_emotion"],
            }

            # Build the system prompt for Claude
            system_prompt = """
            You are an AI system that evaluates customer service conversations to determine if they should be transferred to a human agent.

            TRANSFER CRITERIA:
            1. Urgent issues that require immediate attention ("urgent", "immediately", "right now", "asap", etc.)
            2. Emotional distress (high anger, anxiety, frustration, or any negative emotion)
            3. Complex technical or account issues beyond AI capabilities
            4. Explicit requests for human assistance
            5. Security or privacy concerns
            6. Billing/payment disputes
            7. Escalating frustration across multiple messages
            8. Confusion that persists after clarification attempts

            Analyze the conversation carefully and provide structured output indicating whether transfer is needed, with detailed reasoning and emotional analysis.
            """

            # Get the conversation text for Claude to analyze
            conversation_text = conversation.get_conversation_text()
            if verbose:
                print("*" * 50)
                print(
                    "Going to send the following conversation to Claude MCP for evaluation:"
                )
                print(conversation_text)

            # Use Claude MCP to get a structured decision
            mcp_response = self.claude_client.get_structured_response(
                system_prompt=system_prompt,
                user_message=f"Please analyze this customer service conversation and determine if it should be transferred to a human agent:\n\n{conversation_text}",
                response_schema=response_schema,
            )

            if verbose:
                print("Claude MCP response:")
                print(json.dumps(mcp_response, indent=2))
                print("*" * 50)

            return mcp_response

        def _build_hf_prompt(
            self, conversation: Conversation, is_transfer=False, verbose=False
        ) -> str:
            """
            Build a prompt for a light Hugging Face model based on conversation history
            Gets called by generate_response below
            """

            system_instruction = """
            You are a helpful and professional customer service AI assistant for a major company.
            Your job is to address customer concerns politely and efficiently. Given the attached conversation history,
            generate 1-3 sentences to reply to the user. Don't generate a communication script, just provide a solo response.
            DO NOT mention any phone numbers or provide meta-commentary about AI capabilities.
            """

            if is_transfer:
                system_instruction += " The customer needs urgent human assistance. Just tell them you're connecting them to a human agent."

            prompt = f"<|system|>\n{system_instruction}\n</|system|>\n\n"

            # Add conversation history
            for message in conversation.history:
                if message.sender == "customer":
                    prompt += f"<|user|>\n{message.content}\n</|user|>\n\n"
                elif message.sender == "agent":
                    prompt += f"<|assistant|>\n{message.content}\n</|assistant|>\n\n"

            # Add prefix for the new response
            if conversation.history and conversation.history[-1].sender == "customer":
                prompt += "<|assistant|>\n"

            # if verbose:
            #     print("=" * 50)
            #     print("Created prompt:")
            #     print(prompt)
            #     print("=" * 50)

            return prompt

        def generate_response(
            self, conversation: Conversation, is_transfer=False, verbose=False
        ) -> str:
            """
            Generate a response using a light Hugging Face model based on the conversation

            First calls _build_hf_prompt to generate a prompt for the chosen Hugging Face model
            The invokes the model on that prompt and decodes the model's response

            Args:
                conversation: The customer conversation
                is_transfer: Whether this is a transfer message

            Returns:
                Generated response text
            """
            # return "Ok, tell me more about your situation"
            # print("Creating prompt")
            # Build prompt for Hugging Face model
            prompt = self._build_hf_prompt(conversation, is_transfer, verbose)

            # print("Tokenizing input")
            # Tokenize the prompt
            inputs = self.tokenizer(
                prompt,
                return_tensors="pt",
                padding=True,
                truncation=True,
                return_attention_mask=True,
            ).to(self.device)

            # print("Generating Response")
            # Generate a response
            with torch.no_grad():
                outputs = self.model.generate(
                    inputs["input_ids"],
                    attention_mask=inputs["attention_mask"],
                    max_new_tokens=50 if is_transfer else 100,  # Shorter for transfer
                    temperature=0.7,
                    do_sample=True,
                    num_beams=1,
                    pad_token_id=self.tokenizer.pad_token_id or self.tokenizer.eos_token_id,
                )

            # print("Decoding response")
            # Decode the generated text
            response = self.tokenizer.decode(
                outputs[0][inputs["input_ids"].shape[1] :], skip_special_tokens=True
            ).strip()

            # Clean up the response
            def get_text_before_first_tag(text):

                open_tag_index = text.find("<|")  # Get index of first <|
                close_tag_index = text.find("</|")  # Get index of first </|

                # If both are -1, return the whole text
                if open_tag_index == -1 and close_tag_index == -1:
                    return text

                # If both are positive, return substring(0, smaller_index)
                if open_tag_index != -1 and close_tag_index != -1:
                    return text[: min(open_tag_index, close_tag_index)]

                # If only one is positive, return substring(0, positive_index)
                return text[: open_tag_index if open_tag_index != -1 else close_tag_index]

            response = get_text_before_first_tag(response)

            if not response:
                print("Warning: Generated an empty response")
                response = None

            # if verbose:
            # print("#" * 50)
            # print("Generated response:")
            # print(response)
            # print("#" * 50)

            return response

        def process_message(
            self, conversation: Conversation, new_message: str = None, verbose=False
        ) -> Tuple[str, bool, Dict[str, Any]]:
            """
            Process a new customer message and determine if transfer is needed

            Args:
                conversation: The ongoing conversation
                new_message: New message from customer (if not already in history)

            Returns:
                Tuple of (response generated by Hugging Face model, needs_transfer, mcp_evaluation)
            """
            # Add the new message to conversation if provided
            if new_message:
                conversation.add_message("customer", new_message)

            # Use Claude MCP to evaluate transfer need
            mcp_evaluation = self.evaluate_transfer_need(conversation, verbose=verbose)

            # Check if we need to transfer
            needs_transfer = mcp_evaluation.get("transfer_needed", False)

            # Generate appropriate response with the Hugging Face model
            response = self.generate_response(
                conversation, is_transfer=needs_transfer, verbose=verbose
            )

            # Add the response to conversation history
            conversation.add_message("agent", response)

            return response, needs_transfer, mcp_evaluation

    def run_test_conversation(
        agent, conversation_messages, expected_transfer_point, verbose=False
    ):
        """
        Run a test conversation and check if transfer happens at the expected point

        Args:
            agent: The CustomerServiceAgent instance
            conversation_messages: List of customer messages to process sequentially
            expected_transfer_point: At which message transfer should occur (1-indexed)
            verbose: Whether to print detailed progress

        Returns:
            Tuple of (passed, actual_transfer_point, conversation)
        """
        conversation = Conversation(conversation_id=f"test-{id(conversation_messages)}")
        transfer_occurred = False
        actual_transfer_point = None

        if verbose:
            print(
                f"\n*** --- Expected transfer at message {expected_transfer_point} --- ***\n"
            )

        for i, message in enumerate(conversation_messages, 1):
            if verbose:
                print(f"\nCustomer (message {i}): {message}")

            response, needs_transfer, transfer_details = agent.process_message(
                conversation, message, verbose=False
            )  # setting verbose=true would print out all model responses every step of the way

            if verbose:
                print(f"Agent: {response}")
                print(f"Transfer needed: {needs_transfer}")
                if needs_transfer:
                    print(
                        f"\nReasoning: {transfer_details.get('reasoning', 'No reasoning provided')}"
                    )
                    emotion = transfer_details.get("detected_emotion", {})
                    print(
                        f"\nDetected emotion: {emotion.get('primary_emotion', 'unknown')} (intensity: {emotion.get('intensity', 'unknown')})"
                    )

            if needs_transfer and not transfer_occurred:
                transfer_occurred = True
                actual_transfer_point = i

                if verbose:
                    print(f"\n🔀 TRANSFER OCCURRED AT MESSAGE {i} 🔀")

                # No need to continue the conversation after transfer
                break

        test_passed = actual_transfer_point == expected_transfer_point

        if test_passed:
            print(
                f"\n✅ TEST PASSED: Transfer occurred at expected message {expected_transfer_point}"
            )
        else:
            print(
                f"\n❌ TEST FAILED: Expected transfer at message {expected_transfer_point}, but "
            )
            if actual_transfer_point:
                print(f"transfer occurred at message {actual_transfer_point}")
            else:
                print("no transfer occurred")

        print(f"{'='*80}")

        return test_passed, actual_transfer_point, conversation

    def run_multiple_tests(agent, file_path):

        # Get the file path relative to the current script
        test_file_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), file_path)

        # Load test conversations from JSON file
        try:
            with open(test_file_path, "r") as json_file:
                test_data = json.load(json_file)
        except FileNotFoundError:
            print("Error: test_conversations.json file not found")
            sys.exit(1)
        except json.JSONDecodeError:
            print("Error: Invalid JSON in test_conversations.json")
            sys.exit(1)

        # Run all tests 4 times to demonstrate reproducible results
        all_runs_results = []

        for run_num in range(1, 5):  # 4 runs
            print(f"\n{'*'*100}\nTEST RUN {run_num} OF 4\n{'*'*100}")

            # Create results dictionary to track outcomes for this run
            results = {"run": run_num, "pass": 0, "fail": 0, "tests": []}

            # Process each test group
            for group_name, conversations in test_data.items():
                print(f"\n{'='*80}\nTEST GROUP: {group_name}\n{'='*80}")

                for test_case in conversations:
                    test_id = test_case.get("id", "unknown")
                    expected_transfer = test_case.get("expected_transfer_point")
                    messages = test_case.get("messages", [])

                    print(f"\n{'-'*40}\nRunning test: {test_id}\n{'-'*40}")
                    test_passed, actual_transfer, conversation = run_test_conversation(
                        agent, messages, expected_transfer, verbose=True
                    )

                    # Track results
                    results["tests"].append(
                        {
                            "id": test_id,
                            "group": group_name,
                            "expected_transfer": expected_transfer,
                            "actual_transfer": actual_transfer,
                            "passed": test_passed,
                            "message_count": len(messages),
                        }
                    )

                    if test_passed:
                        results["pass"] += 1
                    else:
                        results["fail"] += 1

            # Store results for this run
            all_runs_results.append(results)

            # Summary for this run
            print(f"\n{'*'*100}\nRUN {run_num} RESULTS SUMMARY\n{'*'*100}")
            print(f"Total tests: {results['pass'] + results['fail']}")
            print(f"Passed: {results['pass']}")
            print(f"Failed: {results['fail']}")

            if results["fail"] > 0:
                print("\nFailed tests in this run:")
                for test in results["tests"]:
                    if not test["passed"]:
                        print(f"- {test['id']} (group: {test['group']})")
                        print(
                            f"  Expected transfer at message {test['expected_transfer']}, got {test['actual_transfer'] or 'no transfer'}"
                        )

        # Overall summary across all runs
        print(f"\n{'='*100}\nOVERALL RESULTS ACROSS ALL 4 RUNS\n{'='*100}")

        # Count total passes and fails
        total_tests = sum(run["pass"] + run["fail"] for run in all_runs_results)
        total_passes = sum(run["pass"] for run in all_runs_results)
        total_fails = sum(run["fail"] for run in all_runs_results)

        print(f"Total tests executed: {total_tests}")
        print(f"Total passes: {total_passes}")
        print(f"Total fails: {total_fails}")
        print(f"Overall pass rate: {(total_passes/total_tests)*100:.2f}%")

        # Check for consistency across runs
        print("\nConsistency analysis:")
        all_test_ids = {test["id"] for run in all_runs_results for test in run["tests"]}

        for test_id in sorted(all_test_ids):
            # Collect results for this test across all runs
            test_results = []
            for run in all_runs_results:
                for test in run["tests"]:
                    if test["id"] == test_id:
                        test_results.append(test)
                        break

            # Check if all runs produced the same result for this test
            if all(r["passed"] == test_results[0]["passed"] for r in test_results):
                result = (
                    "Consistent: All passed"
                    if test_results[0]["passed"]
                    else "Consistent: All failed"
                )
            else:
                result = "Inconsistent results"

            print(f"- {test_id}: {result}")
        return

    # Example usage
    if __name__ == "__main__":
        # Load API key from environment
        from dotenv import load_dotenv

        load_dotenv()

        anthropic_api_key = os.environ.get("ANTHROPIC_API_KEY")
        if not anthropic_api_key:
            print("Error: ANTHROPIC_API_KEY not found in environment")
            sys.exit(1)

        # Initialize the agent with a light Hugging Face model for testing
        model_name = "HuggingFaceTB/SmolLM-1.7B-Instruct"
        agent = CustomerServiceAgent(
            anthropic_api_key=anthropic_api_key,
            model_name=model_name,
        )

        # Example test conversation that should trigger transfer on 2nd message
        test_conversation_1 = [
            "I have two accounts with your service. Can I merge them into one?",
            "Both accounts have subscription history and saved information I want to keep.",
            "I tried the account settings page, but there's no option to merge accounts there.",
            "This is beyond frustrating! I've been trying to solve this simple problem for DAYS! I need to speak to someone with actual authority who can merge these accounts IMMEDIATELY!",
        ]
        run_test_conversation(
            agent, test_conversation_1, expected_transfer_point=3, verbose=True
        )

        # To run all the tests
        file_path = "test_conversations.json"
        # run_multiple_tests(agent, file_path)

    ```
    </div>

- `test_conversations.json -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```json
    {
        "transfer_on_first_message": [
            {
                "id": "urgent-security-1",
                "expected_transfer_point": 1,
                "messages": [
                    "My account was just hacked! Someone is making purchases RIGHT NOW with my credit card!"
                ]
            },
            {
                "id": "urgent-security-2",
                "expected_transfer_point": 1,
                "messages": [
                    "I just got a fraud alert! Someone's using my account in another country RIGHT NOW!"
                ]
            },
            {
                "id": "urgent-payment-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I was just charged $2000 for something I didn't buy! This is URGENT!"
                ]
            },
            {
                "id": "extremely-angry-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I am FURIOUS with your company! I've been a customer for 10 years and this is OUTRAGEOUS! Fix this IMMEDIATELY!"
                ]
            },
            {
                "id": "critical-deadline-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I need IMMEDIATE assistance! My business account is down and I have a critical presentation in 30 minutes!"
                ]
            },
            {
                "id": "missing-order-urgent",
                "expected_transfer_point": 1,
                "messages": [
                    "I need URGENT help! My order for my wedding is missing and the ceremony is TOMORROW! I'm panicking!"
                ]
            },
            {
                "id": "legal-threat-1",
                "expected_transfer_point": 1,
                "messages": [
                    "I'm contacting my lawyer immediately about this fraudulent charge! This is THEFT and I need to speak to a manager RIGHT NOW!"
                ]
            },
            {
                "id": "medical-urgency",
                "expected_transfer_point": 1,
                "messages": [
                    "I need urgent help! My medical device order hasn't arrived and I'm out of supplies. This is a MEDICAL EMERGENCY!"
                ]
            },
            {
                "id": "explicit-human-request-urgent",
                "expected_transfer_point": 1,
                "messages": [
                    "I need to speak to a HUMAN REPRESENTATIVE immediately! This is regarding an urgent security issue with my account!"
                ]
            },
            {
                "id": "extreme-distress",
                "expected_transfer_point": 1,
                "messages": [
                    "I'm literally shaking with anger right now! Your company has charged me THREE TIMES for the same order! I need this fixed IMMEDIATELY!"
                ]
            }
        ],
        "transfer_on_second_message": [
            {
                "id": "refund-escalation",
                "expected_transfer_point": 2,
                "messages": [
                    "I'd like to request a refund for my recent purchase. The product was defective.",
                    "I don't care about your policy! I've been waiting for TWO WEEKS and I demand a refund NOW! Get me a manager!"
                ]
            },
            {
                "id": "technical-issue-escalation",
                "expected_transfer_point": 2,
                "messages": [
                    "The app keeps crashing when I try to upload my documents. Is there a fix?",
                    "I've tried all your suggestions and NOTHING works! This is costing me business! I need to speak to your technical team immediately!"
                ]
            },
            {
                "id": "sudden-urgency-development",
                "expected_transfer_point": 2,
                "messages": [
                    "What are your business hours for customer support?",
                    "Actually, I just noticed someone has hacked my account! There are purchases I didn't make! I need urgent help RIGHT NOW!"
                ]
            },
            {
                "id": "second-message-human-request",
                "expected_transfer_point": 2,
                "messages": [
                    "Can you tell me how to update my profile information?",
                    "This is too complicated. I need to speak with a HUMAN representative immediately. Please connect me with someone who can help."
                ]
            },
            {
                "id": "health-concern-escalation",
                "expected_transfer_point": 2,
                "messages": [
                    "I'm trying to find information about potential allergens in your products.",
                    "This is a SERIOUS medical concern! I need to speak with someone immediately about this product - my child had a severe reaction and I need to know what's in it RIGHT NOW!"
                ]
            },
            {
                "id": "increasing-frustration-3-messages",
                "expected_transfer_point": 2,
                "messages": [
                    "I can't seem to download my invoice for last month's purchase. Where can I find it?",
                    "I checked there already, but the invoice isn't showing up. Can you send it directly?",
                    "This is RIDICULOUS! I've spent 30 minutes trying to get a simple invoice! I need to speak to someone competent IMMEDIATELY!"
                ]
            },
            {
                "id": "shipping-address-problem",
                "expected_transfer_point": 2,
                "messages": [
                    "How do I change the shipping address for my recent order?",
                    "I tried that but it says it's too late to modify the order since it's being processed.",
                    "This is an EMERGENCY! I won't be at that address and the package will be STOLEN! I need to speak to someone who can actually HELP ME!"
                ]
            },
            {
                "id": "warranty-claim-difficulty",
                "expected_transfer_point": 2,
                "messages": [
                    "How do I make a warranty claim for my broken headphones?",
                    "I filled out the form, but I haven't received any confirmation email yet.",
                    "It's been TWO DAYS and I've heard NOTHING! This is completely unprofessional! I want to speak to a manager about this IMMEDIATELY!"
                ]
            },
            {
                "id": "confused-to-angry-progression",
                "expected_transfer_point": 2,
                "messages": [
                    "I'm not sure I understand the difference between your Basic and Premium plans. Can you explain?",
                    "Thanks, but what exactly does 'priority support' mean in practical terms?",
                    "Why can't you give me a straight answer?! I've asked simple questions and gotten vague responses! I want to speak to someone who actually KNOWS your products!"
                ]
            },
            {
                "id": "escalating-service-complaint",
                "expected_transfer_point": 2,
                "messages": [
                    "The technician missed our appointment window yesterday. Can we reschedule?",
                    "Tomorrow doesn't work for me. I've already taken off work once for this.",
                    "No, Saturday isn't possible either. I need a weekday evening after 6pm.",
                    "This is RIDICULOUS! I've been trying to get this service for THREE WEEKS! Your scheduling system is a NIGHTMARE! I need to speak to a manager IMMEDIATELY about this situation!"
                ]
            }
        ],
        "transfer_on_third_message": [
            {
                "id": "policy-explanation-frustration",
                "expected_transfer_point": 3,
                "messages": [
                    "Can you explain your international shipping policies? I'm trying to order from overseas.",
                    "So you're saying there's no way to get free shipping internationally, even with a premium membership?",
                    "This is such a ripoff! I've spent thousands with your company and you can't even offer reasonable shipping?! I want to speak to someone in charge about this policy IMMEDIATELY!"
                ]
            },
            {
                "id": "prolonged-shipping-issue",
                "expected_transfer_point": 3,
                "messages": [
                    "My order was supposed to arrive last week, but the tracking hasn't updated. Can you check on it?",
                    "The tracking number you provided shows it's still in the warehouse, not shipped.",
                    "I called the shipping company and they said they haven't received the package from you yet.",
                    "This is COMPLETELY UNACCEPTABLE! I've been waiting for TWO WEEKS for an order you claimed was shipped! I demand to speak to a manager RIGHT NOW!"
                ]
            },
            {
                "id": "complex-return-issue",
                "expected_transfer_point": 3,
                "messages": [
                    "How do I return an item that was a gift? I don't have the original receipt.",
                    "The person who bought it used their account but shipped it to my address.",
                    "I tried using the order number they forwarded me, but the system doesn't recognize it.",
                    "I've been trying to return this for THREE WEEKS! This is the worst customer service I've ever experienced! Get me a manager NOW who can actually help me!"
                ]
            },
            {
                "id": "account-merger-complexity",
                "expected_transfer_point": 3,
                "messages": [
                    "I have two accounts with your service. Can I merge them into one?",
                    "Both accounts have subscription history and saved information I want to keep.",
                    "I tried the account settings page, but there's no option to merge accounts there.",
                    "This is beyond frustrating! I've been trying to solve this simple problem for DAYS! I need to speak to someone with actual authority who can merge these accounts IMMEDIATELY!"
                ]
            }
        ]
    }

    ```
    </div>

- `run_hybrid_customer_service_agent.sh -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    #!/bin/bash

    # Get the directory where this script is located
    DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    echo "Starting Hybrid Customer Service Agent test from directory: $DIR"
    echo "Python version:"
    python3 --version

    # Run the Python script
    echo "Running Python script ..."
    python3 "$DIR/hybrid_service_agent.py"

    # Capture the exit code
    exit_code=$?

    echo "Python script completed with exit code: $exit_code"

    # Return the same exit code
    exit $exit_code

    ```
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

- `BUILD file -`
    <div style="max-height: 250px; max-width: 50vw; overflow: auto;">

    ```bash
    load("@rules_python//python:py_binary.bzl", "py_binary")

    py_binary(
        name="hybrid_customer_service_agent",
        srcs=["hybrid_service_agent.py"],
        main="hybrid_service_agent.py",
        deps=[
            "@pypi//python_dotenv",
            "@pypi//torch",
            "@pypi//transformers",
            "@pypi//scipy",
            "//src/models/claude_client:claude_client",
        ],
        data=[
            "test_conversations.json",
            "//:.env",  # Moved from deps to data
        ],
        imports=[
            "",
            ".",
            "../..",  # For imports starting with 'src'
        ],
        visibility=["//visibility:public"],
    )

    `sh_test`(
        name="hybrid_customer_service_agent_test",
        srcs=["run_hybrid_customer_service_agent.sh"],
        data=[
            ":hybrid_customer_service_agent",
        ],
    )

    ```
    </div>

The presented hybrid approach uses SmolLM-1.7B but without any fine-tuning. This doesn’t produce trust-worthy results. The goal here is to lay a foundation for smartly using fine-tuned models alongside Claude models.

For local execution, use: <br>
`bazel run //src/agents/hybrid_customer_service:hybrid_customer_service_agent`

For remote execution, use: <br>
`bazel test //src/agents/hybrid_customer_service:hybrid_customer_service_agent_test`

## **9) Integrating with CI/CD**

To fully utilize our Bazel and NativeLink setup, we can add a CI configuration to run tests and builds remotely. Here's an example GitHub Actions workflow file:

1\. Create .`github/workflows` directory <br>

2\. Create a `ci.yml` file in this directory <br>

3\. Paste this into the file: <br>

<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```yaml
name: AI Monorepo (Bazel) CI

on:
  push:
    branches: [ main ]
  pull_request:
    branches: [ main ]

jobs:
  test:
    runs-on: ubuntu-22.04
    environment: production

    steps:
    - name: Checkout
      uses: actions/checkout@b4ffde65f46336ab88eb53be808477a3936bae11  # v4.1.1

    - name: Setup Bazelisk
      uses: bazel-contrib/setup-bazel@b388b84bb637e50cdae241d0f255670d4bd79f29 # v0.8.1
      with:
        bazelisk-cache: true

    - name: Make all shell scripts executable
      run: |
          find . -name "*.sh" -type f -exec chmod +x {} \;

    - name: Set up environment
      run: |
        echo "ANTHROPIC_API_KEY=${{ secrets.ANTHROPIC_API_KEY }}" > .env
        echo "RUN_API_TESTS=true" >> .env

    - name: Run 1st Set of Tests Excluding Cache Manager Tests # Run standard tests
      shell: bash
      run: |
        bazel test \
          --remote_cache=${{ vars.NATIVELINK_COM_REMOTE_CACHE_URL }} \
          --remote_header=${{ secrets.NATIVELINK_COM_API_HEADER }} \
          --bes_backend=${{ vars.NATIVELINK_COM_BES_URL }} \
          --bes_header=${{ secrets.NATIVELINK_COM_API_HEADER }} \
          --bes_results_url=${{ vars.NATIVELINK_COM_BES_RESULTS_URL }} \
          --remote_header=x-nativelink-project=nativelink-ci \
          --remote_executor=grpcs://scheduler-<nativelink_username>.build-faster.nativelink.net:443 \
          --remote_default_exec_properties="container-image=docker://docker.io/evaanahmed2001/python-bazel-env:amd64-v2" \
          --test_output=all \
          //src/models/claude_client:claude_client_test \
          //src/agents/customer_service:customer_service_agent_test \
          //src/models/huggingface:hugging_face_test \
          //src/agents:hybrid_agent_test \
          //src/training:training_test \
          //src/agents/hybrid_customer_service:hybrid_customer_service_agent_test \

    - name: Run Cache Manager tests
      shell: bash
      run: |
        bazel test \
          --remote_cache=${{ vars.NATIVELINK_COM_REMOTE_CACHE_URL }} \
          --remote_header=${{ secrets.NATIVELINK_COM_API_HEADER }} \
          --bes_backend=${{ vars.NATIVELINK_COM_BES_URL }} \
          --bes_header=${{ secrets.NATIVELINK_COM_API_HEADER }} \
          --bes_results_url=${{ vars.NATIVELINK_COM_BES_RESULTS_URL }} \
          --remote_header=x-nativelink-project=nativelink-ci \
          --remote_executor=grpcs://scheduler-<nativelink_username>.build-faster.nativelink.net:443 \
          --remote_default_exec_properties="container-image=docker://docker.io/evaanahmed2001/python-bazel-env:amd64-v2" \
          --test_output=all \
          //src:cache_manager_test

    - name: Run command line agent test with examples flag
      shell: bash
      run: |
        bazel test \
          --remote_cache=${{ vars.NATIVELINK_COM_REMOTE_CACHE_URL }} \
          --remote_header=${{ secrets.NATIVELINK_COM_API_HEADER }} \
          --bes_backend=${{ vars.NATIVELINK_COM_BES_URL }} \
          --bes_header=${{ secrets.NATIVELINK_COM_API_HEADER }} \
          --bes_results_url=${{ vars.NATIVELINK_COM_BES_RESULTS_URL }} \
          --remote_header=x-nativelink-project=nativelink-ci \
          --remote_executor=grpcs://scheduler-<nativelink_username>.build-faster.nativelink.net:443 \
          --remote_default_exec_properties="container-image=docker://docker.io/evaanahmed2001/python-bazel-env:amd64-v2" \
          --test_output=all \
          --test_arg="--examples" \
          //src/demo:complete_agent_test
```
</div>

### NOTES:
1\. We can’t run the Cache Manager test alongside the test for Train Model since they both read and write from the same location by default. This is why we've a separate step for running Cache Manager tests. <br>
2\. The only test that needs command line arguments is `complete_agent_test` and that’s why it has a dedicated step to run it. <br>
3\. Don’t forget to set up your GitHub environment variables and secrets before running this CI workflow! You’ll need 2 secrets (your Anthropic & NativeLink API keys) and 3 variables (NATIVELINK_COM_BES_RESULTS_URL, NATIVELINK_COM_BES_URL, and NATIVELINK_COM_REMOTE_CACHE_URL). <br>

## **Conclusion: The Future of Enterprise AI Development**

As we've demonstrated in this tutorial, the combination of Anthropic's Model Context Protocol, Bazel build system, NativeLink remote execution, and Hugging Face's ecosystem provides a powerful foundation for enterprise-grade AI development. This stack addresses the key challenges that ambitious AI teams face:

1\. **Scalability**: By leveraging NativeLink's remote execution and caching capabilities, you can scale your AI development to handle larger models and datasets. <br>
2\. **Flexibility**: The ability to combine Claude's reasoning capabilities with specialized models from Hugging Face gives you the best of both worlds—powerful general intelligence and domain-specific expertise. <br>
3\. **Developer Experience**: The monorepo approach streamlines collaboration and ensures that best practices are shared across teams. <br>

For the most ambitious AI teams looking to build homegrown agents and agentic experiences, this stack provides the infrastructure needed to move beyond elementary model consumption to creating truly differentiated AI applications. Whether you're building autonomous systems, specialized assistants, or enterprise knowledge workers, the combination of structured communication protocols, efficient build systems, and remote execution enables a new generation of AI applications.

The path from experimental AI to production-ready systems is challenging, but with the right tools and architecture, it becomes much more manageable. By adopting these practices, your team can focus less on infrastructure challenges and more on the creative aspects of AI development—designing intelligent agents that bring real value to your organization.
