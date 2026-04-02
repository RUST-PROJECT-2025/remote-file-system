#!/bin/bash

# Nome dell'immagine e della rete
IMAGE_NAME="rust-fuse-dev"
NETWORK_NAME="rfs-network"
SERVER_CONT="rfs-server"
CLIENT_CONT="rfs-client"
MOUNT_POINT="/mnt/remote-fs"
LOCAL_STORAGE="$(pwd)/rfs_data"

# Funzione per lo spegnimento pulito (Graceful Shutdown)
stop_containers() {
    echo -e "\n--- Ricevuto segnale di stop. Spegnimento pulito in corso... ---"
    # Chiediamo gentilmente al client di smontare (attiva l'handler Rust)
    docker stop -t 5 $CLIENT_CONT 2>/dev/null
    docker stop -t 2 $SERVER_CONT 2>/dev/null
    
    # Pulizia finale (I dati in $LOCAL_STORAGE NON vengono toccati)
    docker rm -f $CLIENT_CONT $SERVER_CONT 2>/dev/null
    docker network rm $NETWORK_NAME 2>/dev/null
    echo "--- Sistema spento. I tuoi file sono al sicuro in: $LOCAL_STORAGE ---"
    exit 0
}

# Cattura Ctrl+C
trap stop_containers INT

echo "--- 1. Pulizia ambiente precedente ---"
docker rm -f $SERVER_CONT $CLIENT_CONT 2>/dev/null
docker network rm $NETWORK_NAME 2>/dev/null
# creo la cartella per i dati persistenti (se non esiste già)
mkdir -p "$LOCAL_STORAGE"

echo "--- 2. Creazione rete Docker: $NETWORK_NAME ---"
docker network create $NETWORK_NAME

echo "--- 3. Avvio Container SERVER ---"
docker run -d \
    --network $NETWORK_NAME \
    --name $SERVER_CONT \
    -v "$(pwd):/workspace" \
    -v "$LOCAL_STORAGE:/tmp/rfs_storage" \
    $IMAGE_NAME \
    bash -c "RUST_LOG=info cargo run --bin rfs_server"

echo "--- 4. Attesa avvio server (5s) ---"
sleep 5

echo "--- 5. Avvio Container CLIENT (Interattivo) ---"
echo "Premi Ctrl+C per chiudere tutto in modo pulito."
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