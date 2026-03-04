#!/bin/bash

set -ex -o pipefail

TARGET_DIRECTORY="$PWD/.bubbles/images/debian-13"
BUBBLES_DIR=$PWD

mkdir -p $TARGET_DIRECTORY
cd $TARGET_DIRECTORY

$BUBBLES_DIR/oras pull ghcr.io/gonicus/bubbles/vm-image:6c68cf5d36ed12bbb2955407270708f0f0fa7b2c

qemu-img convert -f qcow2 -O raw disk.qcow2 disk.img
truncate -s +15G disk.img
rm disk.qcow2
