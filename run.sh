#!/bin/bash

# Nome dell'immagine e della rete
IMAGE_NAME="rust-fuse-dev"
NETWORK_NAME="rfs-network"
SERVER_CONT="rfs-server"
CLIENT_CONT="rfs-client"
MOUNT_POINT="/mnt/remote-fs"

# Trap per pulire tutto con Ctrl+C
trap "docker rm -f $SERVER_CONT $CLIENT_CONT && docker network rm $NETWORK_NAME; exit" INT

echo "--- 1. Pulizia ambiente precedente ---"
docker rm -f $SERVER_CONT $CLIENT_CONT 2>/dev/null
docker network rm $NETWORK_NAME 2>/dev/null

echo "--- 2. Creazione rete Docker: $NETWORK_NAME ---"
docker network create $NETWORK_NAME

echo "--- 3. Avvio Container SERVER ---"
docker run -d \
    --network $NETWORK_NAME \
    --name $SERVER_CONT \
    -v "$(pwd):/workspace" \
    $IMAGE_NAME \
    bash -c "RUST_LOG=info cargo run --bin rfs_server"

echo "--- 4. Attesa avvio server (5s) ---"
sleep 5

echo "--- 5. Avvio Container CLIENT (Interattivo) ---"
echo "----------------------------------------------------------------"

# Avviamo il client passandogli l'URL del container server
docker run -it --rm \
    --privileged \
    --network $NETWORK_NAME \
    --name $CLIENT_CONT \
    --device /dev/fuse \
    -v "$(pwd):/workspace" \
    $IMAGE_NAME \
    bash -c "mkdir -p $MOUNT_POINT && RUST_LOG=debug cargo run --bin remote_file_system -- --mount-point $MOUNT_POINT --ttl 10 --server-url http://rfs-server:8080"