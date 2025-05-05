---
title: "Fine-tune an LLM on CPUs using Bazel and NativeLink"
tags: ["news", "blog-posts"]
image: https://github.com/user-attachments/assets/b2018e85-b31d-4c65-9dd4-34c8b7527880
slug: finetune-llm-with-bazel-nativelink
pubDate: 2025-05-05
readTime: 20 minutes
---

## **Introduction**

The future of AI development belongs not necessarily to those with the most powerful infrastructure but to those who can extract maximum value from available resources. This tutorial emphasizes CPU-based fine-tuning—demonstrating that with intelligent resource management through NativeLink, impressive results can be achieved without expensive GPU or TPU infrastructure. As compute becomes increasingly costly, competitive advantage will shift toward teams that optimize resource efficiency rather than those deploying state-of-the-art hardware.

This guide demonstrates how to establish an optimized AI development pipeline by integrating several key technologies:<br>
1\. **Bazel Build System**: For efficient repo management. A repo managed by Bazel allows your team to work in a unified codebase while maintaining clean separation of concerns.<br>
2\. **NativeLink**: A remote execution and caching system hosted in your cloud. With NativeLink's remote execution capabilities, you can leverage your cloud resources optimally without wasteful duplication of work.<br>
3\. **Hugging Face Transformers**: For integrating with the rich ecosystem of open-source models that you can run locally or deploy anywhere. The transformers library also provides a sophisticated caching mechanism for optimizing loading model weights.<br>


By completing this tutorial, you'll establish a foundation for a scalable AI workflow suitable for organizations of any size looking to build next-generation AI applications.

## **Setting Up Your Repo with Bazel**

Bazel is a build system designed for mono-repos that allows you to organize code into logical components while maintaining dependency relationships. For AI workloads, this is particularly valuable as it lets you separate model definitions, data processing pipelines, training code, and inference services.

### Prerequisites

1\. Bazel ([installation instructions](https://bazel.build/install)). This demo uses Bazel 8.1.1

2\. NativeLink [Cloud Account](https://app.nativelink.com/) (it’s free to get started, and secure) or [NativeLink 0.6.0](https://github.com/TraceMachina/nativelink/releases/tag/v0.6.0) (Apache-licensed, open source is hard-mode)

### Initial Setup

First, let's create our repo and structure it. Let’s name our repository `finetune-repo`.
1\. Create a directory called `finetune-repo` and open your IDE from within it. Also create a `setup.sh` script. Alternatively, you can use the following shell commands
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```bash
# First cd into your preferred location
mkdir finetune-repo
cd finetune-repo
touch setup.sh
```
</div>

2\. Paste the following code in `setup.sh`:

<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```bash
#!/bin/bash
# Script to create the Finetune Repo project structure

set -e  # Exit on any error

echo "Creating Finetune Repo directory structure..."

# Create top-level files
touch bazel_requirements_lock.txt
touch .bazelrc
touch .gitignore
touch BUILD
touch MODULE.bazel

# Create src directory and subdirectories
mkdir -p src/{cache_manager,training}

# Create files in src/cache_manager
touch src/cache_manager/BUILD
touch src/cache_manager/cache_manager.py
touch src/cache_manager/run_cache_manager.sh

# Create files in src/training
touch src/training/BUILD
touch src/training/train_model.py
touch src/training/run_training.sh

# Make all shell scripts executable
find . -name "*.sh" -exec chmod +x {} \;
echo "Made all shell scripts executable"

echo "Finetune Repo directory structure created successfully!"
echo "Directory structure:"
find . -type f | sort

EOF

echo "Setup complete! Your Finetune Repo is ready for development."

```
</div>

3\. Run the following the commands:
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```bash
chmod +x setup.sh # Make this script an executable
./setup.sh # Run the script
```
</div>

This will create all the files we need in one go and we’ll add content along the way. Your finetune-repo should now have the following structure:
<div style="max-height: 250px; max-width: 50vw; overflow: auto;">

```
|-finetune-repo
  |  |-setup.sh
  |  |-.bazelrc
  |  |-.gitignore
  |  |-BUILD
  |  |-MODULE.bazel
  |  |-bazel_requirements_lock.txt
  |  |-src
  |  |  |-cache_manager
  |  |  |  |-BUILD
  |  |  |  |-cache_manager.py
  |  |  |  |-run_cache_manager.sh
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
anyio==4.9.0
    # for transformers
distro==1.9.0
    # for transformers
httpx==0.28.1
    # for transformers
jiter==0.9.0
    # for transformers
pydantic-core==2.33.0
    # for transformers
pydantic==2.11.0
    # for transformers
sniffio==1.3.1
    # for httpx
httpcore==1.0.7
    # for httpx
h11==0.14.0
    # for httpx
annotated-types==0.7.0
    # for transformers
typing-inspection==0.4.0
    # for transformers
exceptiongroup==1.2.2
    # for transformers
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
bazel-finetune-repo
bazel-out
bazel-bin
bazel-testlogs
.bazelrc
.env
```
</div>

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
<img src="https://github.com/user-attachments/assets/1730c51d-c622-4865-a100-110b55b5180a" width=1000 alt="NativeLink Cloud Quickstart">

## **Important Note About Bazel’s Remote Execution Support**

Bazel supports remote execution for building (compiling) and testing only, **NOT** for running the built executables. To use remote servers for remote build execution via Bazel, a roundabout way is to design tests that just execute the Python binary we want to run. If the binary terminates without any errors, the test passes.

Two of the most popular testing methods in Bazel are `py_test` and `sh_test`:

1\. `py_test` uses the `pytest` package to create tests (convenient, user-friendly)

2\. `sh_test` involves coming up with a shell script that executes the test file (old-school but functional and robust)

When building tests with `py_test`, Bazel would build the tests locally and then execute them in the remote environment. This paves the way for architectural mismatch. For example, NativeLink cloud computers use x86-64 chips while modern day Macs use ARM-64 ones, so a `py_test` compiled on Apple Silicon will never run on cloud computers (majority of them use x86 chips). This is why this demo uses shell script tests, or `sh_test`. For a python file named `<file_name>.py`, there’s a `run_<file_name>.sh` script which loads Python from the execution environment and runs `python3 <file_name>.py`.

The flow then boils down to creating a `sh_test` involving `run_<file_name>.sh` and running the test. The test would execute `run_<file_name>.sh` which in turn would execute `<file_name>.py`.

**DRAWBACK:**

Print statements that track progress (like "Starting to fine tune model" or "Exiting this function") behave differently in remote execution. Unlike local runs where these appear in real-time, remote testing collects all logs on the server and only displays them after test completion when control returns to the local machine. The expected output is still fully preserved - just delayed until the process finishes running.

Let’s move on to the code.


## **1) Training / Fine-Tuning On NativeLink Cloud**

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

    sh_test(
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

<img src="https://github.com/user-attachments/assets/acf21144-b160-447f-846b-c516f0a250e4" width="1000" alt="Model Fine-Tuning Times">

## **2) Caching Model Weights**

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

    sh_test(
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

<img src="https://github.com/user-attachments/assets/a941c3a2-32fd-4014-91e9-1e350774ee9f" width="1000" alt="Time Needed to load Cached Hugging Face Model Using Cache Manager">

Observations:

1\. All three environments were fastest when loading the 66M parameter model instead of loading the smallest 4.4M parameter model

2\. Free version of Google Colab doesn't provide enough RAM to load the 1.7B parameter model. We tried 7 times but the session crashed every time.

3\. NativeLink scales very well with bigger data bandwidths. It took less than 1 second to load a model that the free version of Colab just couldn’t.

## **Conclusion: Optimizing AI Development Through Resource Efficiency**

As demonstrated through this tutorial, the integration of Bazel's repo management, NativeLink's CPU-optimized remote execution, and Hugging Face's transformers library creates a development ecosystem that prioritizes computational efficiency over raw processing power. This approach addresses several critical challenges facing modern AI teams:

1\. **Resource Optimization**: By leveraging NativeLink's intelligent execution and caching on CPU infrastructure, teams can achieve impressive fine-tuning results without the capital expenditure of specialized GPU/TPU hardware.<br>
2\. **Strategic Advantage**: This CPU-focused approach provides a competitive edge through efficient resource utilization, enabling teams to allocate budget toward innovation rather than hardware acquisition.<br>
3\. **Sustainable Scaling**: As models grow in size and complexity, the ability to efficiently distribute workloads across existing CPU infrastructure provides a more sustainable path to scale than continuously upgrading to the latest accelerators.<br>

For forward-thinking AI teams, this infrastructure stack represents a shift from the "bigger is better" hardware arms race toward thoughtful resource utilization. The competitive advantage increasingly belongs to those who can extract maximum value from available compute rather than those who deploy more powerful hardware.


The journey from experimental AI projects to production-grade systems demands both technical sophistication and resource awareness. By adopting this CPU-optimized approach with Bazel and NativeLink, your team can focus less on infrastructure limitations and more on the creative potential of fine-tuned models—developing applications that deliver genuine value while maintaining computational efficiency.
