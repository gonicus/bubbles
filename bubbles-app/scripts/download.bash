#!/bin/bash

set -ex -o pipefail

TARGET_DIRECTORY="$PWD/.bubbles/images/debian-13"
BUBBLES_DIR=$PWD

mkdir -p $TARGET_DIRECTORY
cd $TARGET_DIRECTORY

$BUBBLES_DIR/oras pull ghcr.io/gonicus/bubbles/vm-image:b94678cf8785b82f0147f9a3fd5d5220c0b981a4

qemu-img convert -f qcow2 -O raw disk.qcow2 disk.img
truncate -s +15G disk.img
rm disk.qcow2
