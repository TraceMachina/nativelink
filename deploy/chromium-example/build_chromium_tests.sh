#!/usr/bin/env bash

set -euo pipefail

function fetch_chromium() {
    mkdir -p "${HOME}/chromium"
    cd "${HOME}/chromium"
    fetch --no-history chromium
}

# Based on requirements Ubuntu is the most well supported system
# https://chromium.googlesource.com/chromium/src/+/main/docs/linux/build_instructions.md
if ! grep -q 'ID=ubuntu' /etc/os-release; then
    echo "This system is not running Ubuntu."
    exit 0
fi

if [ -d "${HOME}/chromium/src" ]; then
    echo "Using existing chromium checkout"
    cd "${HOME}/chromium"
    set +e
    gclient sync --no-history
    exit_status=$?
    set -e
    if [ $exit_status -ne 0 ]; then
        echo "Failed to sync, removing files in ${HOME}/chromium"
        rm -rf "${HOME}/chromium/"
        fetch_chromium
    fi

    cd src
else
    echo "This script will modify the local system by adding depot_tools to .bashrc,"
    echo "downloading chrome code base and installing dependencies based on instructions"
    echo "https://chromium.googlesource.com/chromium/src/+/main/docs/linux/build_instructions.md."
    echo "Do you want to continue? (yes/no)"
    read -r answer
    answer=$(echo "$answer" | tr '[:upper:]' '[:lower:]')
    if [[ $answer != "yes" ]]; then
        echo "Exiting."
        # Exit or handle "no" logic here
        exit 0
    fi

    # Add depot_tools to path
    if [[ $PATH != *"/depot_tools"* ]]; then
        cd "${HOME}"
        git clone https://chromium.googlesource.com/chromium/tools/depot_tools.git
        echo "export PATH=${HOME}/depot_tools:$PATH" >> "${HOME}/.bashrc"
        export PATH="${HOME}/depot_tools:$PATH"
    fi

    # Checkout chromium into home directory without history
    fetch_chromium
    cd src

    # Install dependencies required for clients to have on chromium builds
    ./build/install-build-deps.sh
fi

echo "Generating ninja projects"
gn gen --args="use_remoteexec=true use_siso=true" out/Default

# Fetch cache and schedular IP address for passing to ninja
NATIVELINK=$(kubectl get gtw nativelink-gateway -o=jsonpath='{.status.addresses[0].value}')

echo "Starting autoninja build"
SISO_REAPI_ADDRESS=${NATIVELINK}:80 SISO_REAPI_CAS_ADDRESS=${NATIVELINK}:80 SISO_REAPI_INSTANCE='' RBE_service_no_security=true autoninja -v -j 50 -C out/Default cc_unittests
