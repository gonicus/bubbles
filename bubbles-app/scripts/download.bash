#!/bin/bash

set -ex -o pipefail

TARGET_DIRECTORY="$PWD/.bubbles/images/debian-13"
BUBBLES_DIR=$PWD

mkdir -p $TARGET_DIRECTORY
cd $TARGET_DIRECTORY

$BUBBLES_DIR/oras pull ghcr.io/gonicus/bubbles/vm-image:9464fca145e9e1e14e4e481088b72fc4826ebe5b

qemu-img convert -f qcow2 -O raw disk.qcow2 disk.img
truncate -s +15G disk.img
rm disk.qcow2
