#!/bin/bash

MOUNT_POINT="/mnt/remote-fs"
LOCAL_STORAGE="$(pwd)/rfs_data"

docker run -it --rm \
    --privileged \
    --name rfs-client \
    --device /dev/fuse \
    -v "$(pwd):/workspace" \
    rust-fuse-dev \
    bash -c "mkdir -p $MOUNT_POINT && RUST_LOG=debug cargo run --bin remote_file_system -- --mount-point $MOUNT_POINT --ttl 10 --server-url http://192.168.1.133:8080"