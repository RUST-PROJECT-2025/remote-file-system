#!/bin/bash


docker run -d \
    --name server \
    -v "$(pwd):/workspace" \
    -v "$(pwd)/rfs_data:/tmp/rfs_storage" \
    rust-fuse-dev \
    bash -c "RUST_LOG=info cargo run --bin rfs_server"