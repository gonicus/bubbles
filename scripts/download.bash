#!/bin/bash

set -ex -o pipefail

TARGET_DIRECTORY="$PWD/.bubbles/images/debian-13"
BUBBLES_DIR=$PWD

mkdir -p $TARGET_DIRECTORY
cd $TARGET_DIRECTORY

$BUBBLES_DIR/oras pull ghcr.io/gonicus/bubbles/vm-image:836ba7f8fce0d6f3a503f9d7a44687f9d558f71f

qemu-img convert -f qcow2 -O raw disk.qcow2 disk.img
truncate -s +15G disk.img
rm disk.qcow2
