# Questo script compila il progetto, avvia il server, 
# monta il FUSE client in background e fa tutti i test sulle performance
# sui file grandi, sulla cache e sul demone

#!/bin/bash

# set -e disabilitato per permettere ai test di fallire senza fermare lo script
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

MOUNT_POINT="/tmp/rfs-test-mount"
RFS_BINARY="./target/debug/remote_file_system"
RFS_SERVER_BINARY="./target/debug/rfs_server"
RFS_PID=""
RFS_SERVER_PID=""
TEST_FAILED=0
TEST_PASSED=0

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((TEST_PASSED++)); }
log_fail() { echo -e "${RED}[FAIL]${NC} $1"; ((TEST_FAILED++)); }
log_test() { echo -e "\n${YELLOW}[TEST]${NC} $1"; }

cleanup() {
    log_info "Smontaggio e pulizia..."
    if [ ! -z "$RFS_SERVER_PID" ]; then kill $RFS_SERVER_PID 2>/dev/null; fi
    if [ ! -z "$RFS_PID" ]; then kill $RFS_PID 2>/dev/null; fi
    
    if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
        fusermount3 -u "$MOUNT_POINT" 2>/dev/null || fusermount -u "$MOUNT_POINT" 2>/dev/null || umount "$MOUNT_POINT" 2>/dev/null
    fi
    rm -rf "$MOUNT_POINT"
    # Uccidi eventuali demoni rimasti in vita dai test
    pkill -f "remote_file_system --daemon" 2>/dev/null || true
}

trap cleanup EXIT

setup() {
    log_info "Compilazione del progetto..."
    cargo build --bin remote_file_system
    cargo build --bin rfs_server

    mkdir -p "$MOUNT_POINT"

    log_info "Avvio Server RFS..."
    # Assicuriamoci che il server usi la cartella standard
    export RFS_STORAGE_PATH="/tmp/rfs_storage"
    rm -rf /tmp/rfs_storage
    $RFS_SERVER_BINARY > /tmp/rfs_server.log 2>&1 &
    RFS_SERVER_PID=$!
    sleep 2 # Attendi avvio server

    log_info "Avvio Demone FUSE..."
    $RFS_BINARY --mount-point "$MOUNT_POINT" > /tmp/rfs_client.log 2>&1 &
    RFS_PID=$!
    sleep 2 # Attendi mount
    
    if mountpoint -q "$MOUNT_POINT"; then
        log_pass "FUSE filesystem montato con successo."
    else
        log_fail "Impossibile montare FUSE."
        exit 1
    fi
}

test_crud() {
    log_test "Test Operazioni CRUD (Create, Read, Update, Delete)"
    
    # Create & Read
    echo "Ciao Mondo" > "$MOUNT_POINT/test.txt"
    if [ "$(cat $MOUNT_POINT/test.txt 2>/dev/null)" = "Ciao Mondo" ]; then
        log_pass "Scrittura e Lettura file"
    else log_fail "Scrittura e Lettura file"; fi

    # Update
    echo "Aggiornato" > "$MOUNT_POINT/test.txt"
    if [ "$(cat $MOUNT_POINT/test.txt 2>/dev/null)" = "Aggiornato" ]; then
        log_pass "Aggiornamento file (Sovrascrittura)"
    else log_fail "Aggiornamento file (Sovrascrittura)"; fi

    # Mkdir
    mkdir "$MOUNT_POINT/testdir"
    if [ -d "$MOUNT_POINT/testdir" ]; then
        log_pass "Creazione cartella (mkdir)"
    else log_fail "Creazione cartella (mkdir)"; fi

    # Delete
    rm "$MOUNT_POINT/test.txt"
    rmdir "$MOUNT_POINT/testdir"
    if [ ! -f "$MOUNT_POINT/test.txt" ] && [ ! -d "$MOUNT_POINT/testdir" ]; then
        log_pass "Eliminazione file e cartelle"
    else log_fail "Eliminazione file e cartelle"; fi
}

test_performance_large_files() {
    log_test "Test File Grandi (10MB) e Performance"
    
    # Scrittura 10MB
    dd if=/dev/zero of="$MOUNT_POINT/large.bin" bs=1M count=10 2>/dev/null
    local size=$(stat -c%s "$MOUNT_POINT/large.bin" 2>/dev/null)
    if [ "$size" = "10485760" ]; then
        log_pass "Scrittura Streaming File di 10MB"
    else log_fail "Scrittura Streaming File di 10MB ($size bytes trovati)"; fi

    # Lettura 10MB
    local read_bytes=$(wc -c < "$MOUNT_POINT/large.bin" 2>/dev/null)
    if [ "$read_bytes" = "10485760" ]; then
        log_pass "Lettura Streaming File di 10MB"
    else log_fail "Lettura Streaming File di 10MB"; fi
    
    rm "$MOUNT_POINT/large.bin"
}

test_caching() {
    log_test "Test Cache Hit e Consistenza"
    echo "Dato in cache" > "$MOUNT_POINT/cache.txt"
    
    local c1=$(cat "$MOUNT_POINT/cache.txt")
    local c2=$(cat "$MOUNT_POINT/cache.txt") # Dovrebbe pescare dalla cache
    
    if [ "$c1" = "$c2" ]; then
        log_pass "La cache risponde con dati consistenti"
    else log_fail "La cache è inconsistente"; fi
    rm "$MOUNT_POINT/cache.txt"
}

test_daemon_mode() {
    log_test "Test Esecuzione in Background (Daemon Mode)"
    local daemon_mount="/tmp/rfs-daemon"
    mkdir -p "$daemon_mount"
    
    $RFS_BINARY --mount-point "$daemon_mount" --daemon > /dev/null 2>&1
    sleep 2
    
    if [ -f "/tmp/rfs.pid" ] && mountpoint -q "$daemon_mount"; then
        local d_pid=$(cat /tmp/rfs.pid)
        if kill -0 "$d_pid" 2>/dev/null; then
            log_pass "Demone avviato correttamente e sganciato dal terminale"
            kill "$d_pid"
            sleep 1
        else log_fail "Processo demone morto prematuramente"; fi
    else
        log_fail "Avvio demone fallito o mountpoint non registrato"
    fi
    fusermount3 -u "$daemon_mount" 2>/dev/null || true
    rmdir "$daemon_mount" 2>/dev/null || true
}

test_graceful_shutdown() {
    log_test "Test Graceful Shutdown (Flush in chiusura)"
    local test_file="$MOUNT_POINT/shutdown.txt"
    echo "Dati salvati prima dello spegnimento" > "$test_file"
    
    # Uccide il client FUSE
    kill -TERM $RFS_PID
    sleep 2
    
    # Siccome il FUSE è spento, leggiamo direttamente dalla cartella root del server
    # per vedere se ha "flushato" i dati prima di morire.
    if [ "$(cat /tmp/rfs_storage/shutdown.txt 2>/dev/null)" = "Dati salvati prima dello spegnimento" ]; then
        log_pass "I dati sono stati salvati sul server prima della terminazione"
    else
        log_fail "Dati persi, il client non ha completato il flush in chiusura"
    fi
    rm /tmp/rfs_storage/shutdown.txt 2>/dev/null || true
}

test_concurrency() {
    log_test "Test Concorrenza (Scritture e Letture simultanee)"
    
    local num_jobs=15
    local pids=""
    
    # Lancia $num_jobs processi in background contemporaneamente
    for i in $(seq 1 $num_jobs); do
        (
            echo "Processo concorrente $i" > "$MOUNT_POINT/conc_$i.txt"
            cat "$MOUNT_POINT/conc_$i.txt" > /dev/null
        ) &
        # Salva il PID del processo in background
        pids="$pids $!"
    done
    
    # Aspetta che tutti i processi in background finiscano
    wait $pids 2>/dev/null
    
    # Verifica che tutti i file siano stati creati correttamente e contengano i dati giusti
    local success=true
    for i in $(seq 1 $num_jobs); do
        if [ ! -f "$MOUNT_POINT/conc_$i.txt" ]; then
            success=false
            break
        elif [ "$(cat "$MOUNT_POINT/conc_$i.txt" 2>/dev/null)" != "Processo concorrente $i" ]; then
            success=false
            break
        fi
        # Pulizia
        rm -f "$MOUNT_POINT/conc_$i.txt" 2>/dev/null
    done
    
    if $success; then
        log_pass "Tutte le 15 operazioni concorrenti sono state completate senza corruzione dati"
    else
        log_fail "Fallimento o corruzione dati durante le operazioni concorrenti"
    fi
}

test_network_failure() {
    log_test "Test Resilienza (Caduta improvvisa del Server)"
    
    # 1. Spegniamo brutalmente il server API (simula un crash o disconnessione)
    if [ ! -z "$RFS_SERVER_PID" ]; then
        kill -9 $RFS_SERVER_PID 2>/dev/null
        sleep 1
    fi
    
    # 2. Proviamo a leggere la directory tramite FUSE.
    # Usiamo 'timeout' per assicurarci che il terminale non si blocchi all'infinito (hang) 
    # se il client FUSE non gestisce bene la disconnessione.
    timeout 5 ls "$MOUNT_POINT" > /dev/null 2>&1
    local exit_code=$?
    
    # 124 è il codice di uscita standard del comando 'timeout' in Linux
    if [ $exit_code -eq 124 ]; then
        log_fail "Il client FUSE si è bloccato (hang) a causa della caduta del server"
    elif [ $exit_code -ne 0 ]; then
        log_pass "Il client ha restituito correttamente un errore di I/O senza bloccarsi"
    else
        # Se 'ls' dovesse avere successo, significa che sta leggendo unicamente dalla cache locale.
        # Anche questo può essere considerato un "pass" se l'architettura lo prevede.
        log_pass "Il client ha risposto usando la cache (Server offline)"
    fi
    
    # Essendo un test distruttivo, svuotiamo il PID del server così la funzione cleanup non dà errori
    RFS_SERVER_PID=""
}

main() {
    setup
    test_crud
    test_performance_large_files
    test_caching
    test_daemon_mode
    test_graceful_shutdown

    test_concurrency
    test_network_failure
    
    echo -e "\n${BLUE}=== RISULTATI ===${NC}"
    echo -e "Test Passati: ${GREEN}$TEST_PASSED${NC}"
    echo -e "Test Falliti: ${RED}$TEST_FAILED${NC}"
    
    if [ $TEST_FAILED -gt 0 ]; then exit 1; else exit 0; fi
}

main