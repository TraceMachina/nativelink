#!/bin/bash
# Copyright 2022 The NativeLink Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Creates a raid from all available drives if multiple drives.
# Then creates an ext4 filesystem on raid (or single drive if no raid).

set -euxo pipefail

if [[ $EUID -ne 0 ]]; then
   echo "This script must be run as root"
   exit 1
fi

MOUNT_LOCATION="$1"
shift

DEVICES=( $(lsblk --fs --json | jq -r '.blockdevices[] | select(.children == null and .fstype == null and .mountpoints[0] == null) | .name') )
DEVICES_FULLNAME=()
for DEVICE in "${DEVICES[@]}"; do
  DEVICES_FULLNAME+=("/dev/$DEVICE")
done

rm -rf "$MOUNT_LOCATION"
mkdir -p "$MOUNT_LOCATION"

if [ "${#DEVICES_FULLNAME[@]}" -eq "0" ]; then
    echo "No unused drives, using root volume"
    exit 0
elif [ "${#DEVICES_FULLNAME[@]}" -eq "1" ]; then
    echo "Only one volume letting ext4 manage it directly instead of raid"
    DEVICE="${DEVICES_FULLNAME[@]}"
else
    mdadm --create --verbose /dev/md0 --level=0 --raid-devices=${#DEVICES_FULLNAME[@]} "${DEVICES_FULLNAME[@]}"
    DEVICE="/dev/md0"
fi
mkfs.ext4 "$DEVICE"
mount "$DEVICE" "$MOUNT_LOCATION"
