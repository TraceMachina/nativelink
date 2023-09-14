#!/bin/sh

# This script is run before a job is downloaded to the worker to execute to
# ensure that the worker is in a state that it can accept the job.  If it
# refuses the job it will be re-scheduled and this worker will not get another
# job offered to it until an existing job completes.  If there are no jobs
# running the next job will be offered again immediately.

set -eu

AVAILABLE_MEMORY_KB=$(awk '/MemAvailable/ { printf "%d \n", $2 }' /proc/meminfo);
# At least 2Gb of RAM currently available.
if [ $AVAILABLE_MEMORY_KB -gt 2000000 ]; then
    exit 0
else
    echo "Available memory: ${AVAILABLE_MEMORY_KB}";
    exit 1
fi
