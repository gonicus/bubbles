#!/bin/bash

set -ex -o pipefail

TARGET_DIRECTORY="$PWD/.bubbles/images/debian-13"
BUBBLES_DIR=$PWD

mkdir -p $TARGET_DIRECTORY
cd $TARGET_DIRECTORY

$BUBBLES_DIR/oras pull ghcr.io/gonicus/bubbles/vm-image:e289a3a5479817c3ffad6bb62d8214e4265e8e4b

qemu-img convert -f qcow2 -O raw disk.qcow2 disk.img
truncate -s +15G disk.img
rm disk.qcow2
