#!/bin/sh

AVAILABLE_MEMORY_KB=$(awk '/MemAvailable/ { printf "%d \n", $2 }' /proc/meminfo);
# At least 2Gb of RAM currently available
if [ $AVAILABLE_MEMORY_KB -gt 2000000 ]; then
    exit 0
else
    echo "Available memory: ${AVAILABLE_MEMORY_KB}";
    exit 1
fi
