#!/bin/bash

##############################################################################
# Remote File System - Comprehensive Test Suite
# Tests compliance with project specification
##############################################################################

# set -e removed to allow tests to continue even if some fail
# set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
MOUNT_POINT="/tmp/rfs-test-mount"
SERVER_STORAGE="/tmp/rfs-test-storage" # Cartella di storage separata per il server
RFS_BINARY="./target/debug/remote_file_system"
RFS_SERVER_BINARY="./target/debug/rfs_server"
RFS_PID=""
RFS_SERVER_PID=""
TEST_RESULTS=0
TEST_PASSED=0
# Only use SKIP_SETUP if explicitly passed from environment
# Default to 0 (do full setup) to ensure clean test runs
SKIP_SETUP=${SKIP_SETUP:-0}

##############################################################################
# Utility Functions
##############################################################################

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_pass() {
    echo -e "${GREEN}[PASS]${NC} $1"
    ((TEST_PASSED++))
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $1"
    ((TEST_RESULTS++))
}

log_test() {
    echo -e "${YELLOW}[TEST]${NC} $1"
}

cleanup() {
    log_info "Cleaning up..."
    if [ "$SKIP_SETUP" = "1" ]; then
        log_info "SKIP_SETUP=1 - leaving existing daemon and mount intact"
        return
    fi

    # Kill RFS server if we started it
    if [ ! -z "$RFS_SERVER_PID" ] && kill -0 $RFS_SERVER_PID 2>/dev/null; then
        log_info "Stopping RFS server (PID: $RFS_SERVER_PID)"
        kill -9 $RFS_SERVER_PID 2>/dev/null || true
        sleep 1
    fi

    # Kill FUSE daemon (solo come fallback se l'unmount fallisce)
    if [ ! -z "$RFS_PID" ] && kill -0 $RFS_PID 2>/dev/null; then
        log_info "Stopping FUSE daemon (PID: $RFS_PID)"
        kill -9 $RFS_PID 2>/dev/null || true
        sleep 1
    fi

    if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
        log_info "Unmounting $MOUNT_POINT"
        if command -v fusermount3 >/dev/null 2>&1; then
            fusermount3 -u "$MOUNT_POINT" 2>/dev/null || true
        elif command -v fusermount >/dev/null 2>&1; then
            fusermount -u "$MOUNT_POINT" 2>/dev/null || true
        else
            umount "$MOUNT_POINT" 2>/dev/null || true
        fi
        sleep 1
    fi

    if [ -d "$MOUNT_POINT" ]; then
        rm -rf "$MOUNT_POINT"
    fi
    
    if [ -d "$SERVER_STORAGE" ]; then
        rm -rf "$SERVER_STORAGE"
    fi
}

trap cleanup EXIT

setup() {
    log_info "=== SETUP ==="
    
    # Clean environment
    cleanup 2>/dev/null || true
    
    # Create mount point and server storage
    mkdir -p "$MOUNT_POINT"
    mkdir -p "$SERVER_STORAGE"
    log_pass "Directories created: Mount -> $MOUNT_POINT | Storage -> $SERVER_STORAGE"
    
    # Check if binaries exist
    if [ ! -f "$RFS_BINARY" ]; then
        log_fail "Binary not found: $RFS_BINARY"
        log_info "Building project..."
        cargo build --bin remote_file_system
    fi
    
    if [ ! -f "$RFS_SERVER_BINARY" ]; then
        log_fail "Server binary not found: $RFS_SERVER_BINARY"
        log_info "Building server..."
        cargo build --bin rfs_server
    fi
    
    # Export env var for the server
    export RFS_STORAGE_PATH="$SERVER_STORAGE"
    
    # Start RFS HTTP server first
    log_info "Starting RFS server on http://127.0.0.1:8080..."
    $RFS_SERVER_BINARY > /tmp/rfs_server.log 2>&1 &
    RFS_SERVER_PID=$!
    
    # Wait for server to be ready
    local max_attempts=15
    local attempt=0
    while [ $attempt -lt $max_attempts ]; do
        if curl -s http://127.0.0.1:8080/api/list/ > /dev/null 2>&1; then
            log_pass "RFS server is ready"
            sleep 1
            break
        fi
        sleep 1
        ((attempt++))
    done

    if [ $attempt -eq $max_attempts ]; then
        log_fail "RFS server failed to start"
        return 1
    fi
    
    # Start FUSE daemon
    log_info "Starting FUSE daemon..."
    $RFS_BINARY --mount-point "$MOUNT_POINT" > /tmp/rfs_test.log 2>&1 &
    RFS_PID=$!
    
    # Wait for mount
    local max_attempts=10
    local attempt=0
    while [ $attempt -lt $max_attempts ]; do
        if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
            log_pass "FUSE filesystem mounted at $MOUNT_POINT"
            sleep 1
            return 0
        fi
        sleep 1
        ((attempt++))
    done
    
    log_fail "Failed to mount FUSE filesystem"
    return 1
}

##############################################################################
# Test Suite 1: Basic Mount/Unmount
##############################################################################

test_mount_unmount() {
    log_test "Mount/Unmount Operations"
    
    if mountpoint -q "$MOUNT_POINT"; then
        log_pass "FUSE filesystem is mounted"
    else
        log_fail "FUSE filesystem is not mounted"
        return 1
    fi
}

##############################################################################
# Test Suite 2: Basic File Operations (CRUD)
##############################################################################

test_create_file() {
    log_test "Create File Operation"
    
    local test_file="$MOUNT_POINT/test_create_${RANDOM}.txt"
    echo "Hello World" > "$test_file"
    
    if [ -f "$test_file" ]; then
        log_pass "File created successfully"
    else
        log_fail "File creation failed"
        return 1
    fi
}

test_read_file() {
    log_test "Read File Operation"
    
    local test_file="$MOUNT_POINT/test_read_${RANDOM}.txt"
    echo "Hello World" > "$test_file"
    local content=$(cat "$test_file" 2>/dev/null)
    
    if [ "$content" = "Hello World" ]; then
        log_pass "File read successfully: '$content'"
    else
        log_fail "File read failed or content mismatch"
        return 1
    fi
}

test_update_file() {
    log_test "Update File Operation"
    
    local test_file="$MOUNT_POINT/test_update_${RANDOM}.txt"
    echo "Initial Content" > "$test_file"
    echo "Updated Content" > "$test_file"
    
    local content=$(cat "$test_file" 2>/dev/null)
    if [ "$content" = "Updated Content" ]; then
        log_pass "File update successful"
    else
        log_fail "File update failed"
        return 1
    fi
}

test_append_file() {
    log_test "Append to File Operation"
    
    local test_file="$MOUNT_POINT/test_append_${RANDOM}.txt"
    
    # 1. Scrittura iniziale
    echo "Test 1 Prima riga" > "$test_file"
    
    # 2. Append (nota il doppio >>)
    echo "Seconda riga test" >> "$test_file"
    
    # 3. Lettura del file
    local content=$(cat "$test_file" 2>/dev/null)
    local line_count=$(wc -l < "$test_file")
    
    # Stampiamo visivamente il contenuto per controllo
    log_info "Contenuto letto dal file dopo l'append:"
    echo "$content"
    
    # Verifica: ci aspettiamo esattamente 2 righe e la presenza di entrambe le frasi
    if [ "$line_count" -eq 2 ] && echo "$content" | grep -q "Test 1 Prima riga" && echo "$content" | grep -q "Seconda riga test"; then
        log_pass "File append successful"
    else
        log_fail "File append failed. Contenuto inatteso."
        return 1
    fi
}

test_delete_file() {
    log_test "Delete File Operation"
    
    local test_file="$MOUNT_POINT/test_delete_${RANDOM}.txt"
    echo "To be deleted" > "$test_file"
    rm "$test_file"
    
    if [ ! -f "$test_file" ]; then
        log_pass "File deleted successfully"
    else
        log_fail "File deletion failed"
        return 1
    fi
}

test_mkdir() {
    log_test "Create Directory Operation"
    
    local test_dir="$MOUNT_POINT/testdir_${RANDOM}"
    mkdir "$test_dir"
    
    if [ -d "$test_dir" ]; then
        log_pass "Directory created successfully"
    else
        log_fail "Directory creation failed"
        return 1
    fi
}

test_rmdir() {
    log_test "Remove Directory Operation"
    
    local test_dir="$MOUNT_POINT/testdir_rm_${RANDOM}"
    mkdir "$test_dir"
    rmdir "$test_dir"
    
    if [ ! -d "$test_dir" ]; then
        log_pass "Directory removed successfully"
    else
        log_fail "Directory removal failed"
        return 1
    fi
}

test_list_directory() {
    log_test "List Directory Contents"
    
    local test_dir="$MOUNT_POINT/testdir_list_${RANDOM}"
    mkdir "$test_dir"
    local content1="$test_dir/file1.txt"
    local content2="$test_dir/file2.txt"
    
    echo "Content 1" > "$content1"
    echo "Content 2" > "$content2"
    
    local listing=$(ls "$test_dir" | wc -l)
    
    if [ $listing -ge 2 ]; then
        log_pass "Directory listing works (found $listing entries)"
    else
        log_fail "Directory listing failed"
        return 1
    fi
}

##############################################################################
# Test Suite 3: File Attributes
##############################################################################

test_file_size() {
    log_test "File Size Attribute"
    
    local test_file="$MOUNT_POINT/size_test_${RANDOM}.txt"
    echo "1234567890" > "$test_file"  # 11 bytes (including newline)
    
    local size=$(stat -c%s "$test_file" 2>/dev/null || stat -f%z "$test_file" 2>/dev/null)
    
    if [ $size -gt 0 ]; then
        log_pass "File size attribute readable: $size bytes"
    else
        log_fail "File size attribute not readable"
        return 1
    fi
}

test_file_permissions() {
    log_test "File Permissions Attribute"
    
    local test_file="$MOUNT_POINT/perm_test_${RANDOM}.txt"
    echo "test" > "$test_file"
    chmod 644 "$test_file"
    
    local perms=$(stat -c%a "$test_file" 2>/dev/null || stat -f%A "$test_file" 2>/dev/null)
    
    if [ -n "$perms" ]; then
        log_pass "File permissions readable: $perms"
    else
        log_fail "File permissions not readable"
        return 1
    fi
}

test_file_timestamps() {
    log_test "File Timestamps Attribute"
    
    local test_file="$MOUNT_POINT/time_test_${RANDOM}.txt"
    echo "test" > "$test_file"
    
    local timestamp=$(stat -c%y "$test_file" 2>/dev/null || stat -f "%Sm" "$test_file" 2>/dev/null)
    
    if [ -n "$timestamp" ]; then
        log_pass "File timestamp readable"
    else
        log_fail "File timestamp not readable"
        return 1
    fi
}

##############################################################################
# Test Suite 4: Large Files (Streaming Support 200MB+ & Latenza)
##############################################################################

test_large_file_write() {
    log_test "Large File Write (200MB+ Streaming & Latenza)"
    
    local test_file="$MOUNT_POINT/large_file_${RANDOM}.bin"
    
    log_info "Writing 200MB file... (Questo testerà il buffering e lo streaming)"
    
    # Iniziamo a misurare un attimo prima dell'operazione di I/O
    local start_time=$(date +%s%3N)
    
    # Usiamo urandom/zero per generare 200 blocchi da 1MB
    dd if=/dev/zero of="$test_file" bs=1M count=200 2>/dev/null
    
    # Fermiamo il cronometro appena il comando finisce
    local end_time=$(date +%s%3N)
    local elapsed=$((end_time - start_time))
    
    if [ -f "$test_file" ]; then
        local size=$(stat -c%s "$test_file" 2>/dev/null || stat -f%z "$test_file" 2>/dev/null)
        local expected=$((200 * 1024 * 1024))
        
        if [ $size -eq $expected ]; then
            # Calcolo approssimativo del throughput in MB/s (200MB / secondi)
            # Moltiplichiamo per 1000 per gestire la divisione intera con i millisecondi
            local throughput=0
            if [ $elapsed -gt 0 ]; then
                throughput=$(( 200000 / elapsed ))
            fi
            
            log_pass "File da 200MB scritto in ${elapsed}ms (Throughput: ~${throughput} MB/s)"
        else
            log_fail "Mismatch dimensione file grande: atteso $expected, trovato $size"
        fi
    else
        log_fail "Scrittura file da 200MB fallita"
    fi
}

test_large_file_read() {
    log_test "Large File Read (200MB+ Streaming & Latenza)"
    
    local test_file="$MOUNT_POINT/large_read_${RANDOM}.bin"
    
    # Creiamo preventivamente il file da 200MB (non lo contiamo nella latenza di lettura!)
    dd if=/dev/zero of="$test_file" bs=1M count=200 2>/dev/null
    
    log_info "Reading 200MB file... (Questo testerà il prefetching)"
    
    # Iniziamo a misurare
    local start_time=$(date +%s%3N)
    
    # wc -c forza la lettura sequenziale di tutto il file da cima a fondo
    local read_bytes=$(wc -c < "$test_file")
    
    # Fermiamo il cronometro
    local end_time=$(date +%s%3N)
    local elapsed=$((end_time - start_time))
    
    local expected=$((200 * 1024 * 1024))
    
    if [ $read_bytes -eq $expected ]; then
        local throughput=0
        if [ $elapsed -gt 0 ]; then
            throughput=$(( 200000 / elapsed ))
        fi
        
        log_pass "File da 200MB letto in ${elapsed}ms (Throughput: ~${throughput} MB/s)"
    else
        log_fail "Mismatch lettura file grande: atteso $expected, letti $read_bytes"
    fi
}

##############################################################################
# Test Suite 5: Performance (<500ms Latency Requirement)
##############################################################################

test_strict_latency() {
    log_test "Strict Latency Requirement (<500ms)"
    
    local test_file="$MOUNT_POINT/strict_latency_${RANDOM}.txt"
    
    # Misuriamo il ciclo completo di un'operazione comune: apertura, scrittura e lettura
    local start_time=$(date +%s%3N)
    
    echo "Verifica requisiti non funzionali" > "$test_file"
    cat "$test_file" > /dev/null
    
    local end_time=$(date +%s%3N)
    local elapsed=$((end_time - start_time))
    
    # Il requisito chiede <500ms
    if [ $elapsed -lt 500 ]; then
        log_pass "Latenza operativa I/O: ${elapsed}ms (Requisito <500ms SODDISFATTO)"
    else
        log_fail "Latenza TROPPO ALTA: ${elapsed}ms (Atteso <500ms)"
        return 1
    fi
}

test_directory_listing_speed() {
    log_test "Directory Listing Performance"
    
    local perf_dir="$MOUNT_POINT/perf_dir_${RANDOM}"
    mkdir "$perf_dir"
    
    log_info "Creating 50 test files..."
    for i in {1..50}; do
        echo "file $i" > "$perf_dir/file_$i.txt"
    done
    
    local start_time=$(date +%s%3N)
    ls "$perf_dir" > /dev/null
    local end_time=$(date +%s%3N)
    local elapsed=$((end_time - start_time))
    
    # Un list di 50 file deve stare comodamente sotto i 500ms
    if [ $elapsed -lt 500 ]; then
        log_pass "Listing di 50 file completato in ${elapsed}ms (Requisito <500ms SODDISFATTO)"
    else
        log_fail "Directory listing troppo lento: ${elapsed}ms"
        return 1
    fi
}

test_large_files_io_and_latency() {
    log_test "Large Files I/O & Strict Latency (100MB, 300MB, 500MB)"
    
    local sizes=(100 300 500)
    local all_passed=true
    
    for size in "${sizes[@]}"; do
        local test_file="$MOUNT_POINT/file_${size}MB_${RANDOM}.bin"
        log_info "--- Inizio test su file da ${size} MB ---"
        
        # 1. Scrittura Sequenziale Completa (Throughput)
        local start_write=$(date +%s%3N)
        dd if=/dev/zero of="$test_file" bs=1M count=$size 2>/dev/null
        local end_write=$(date +%s%3N)
        local elapsed_write=$((end_write - start_write))
        
        local throughput_write=0
        if [ $elapsed_write -gt 0 ]; then
            throughput_write=$(( (size * 1000) / elapsed_write ))
        fi
        log_info "Scrittura totale completata in ${elapsed_write}ms (~${throughput_write} MB/s)"
        
        # Verifica che il file esista prima di procedere
        if [ ! -f "$test_file" ]; then
            log_fail "Scrittura fallita: il file da ${size}MB non è stato creato"
            all_passed=false
            continue
        fi

        # 2. Latenza Micro-Lettura (<500ms)
        # Leggiamo solo i primi 4KB. Se il chunking funziona, non scaricherà tutto il file.
        local start_lat_read=$(date +%s%3N)
        head -c 4096 "$test_file" > /dev/null
        local end_lat_read=$(date +%s%3N)
        local elapsed_lat_read=$((end_lat_read - start_lat_read))
        
        if [ $elapsed_lat_read -lt 500 ]; then
            log_pass "Latenza di micro-lettura: ${elapsed_lat_read}ms (Requisito <500ms SODDISFATTO)"
        else
            log_fail "Latenza micro-lettura TROPPO ALTA: ${elapsed_lat_read}ms (Atteso <500ms)"
            all_passed=false
        fi

        # 3. Latenza Micro-Append (<500ms)
        # Aggiungiamo pochi byte alla fine del file gigante
        local start_lat_write=$(date +%s%3N)
        echo "Append test" >> "$test_file"
        local end_lat_write=$(date +%s%3N)
        local elapsed_lat_write=$((end_lat_write - start_lat_write))
        
        if [ $elapsed_lat_write -lt 500 ]; then
            log_pass "Latenza di micro-append: ${elapsed_lat_write}ms (Requisito <500ms SODDISFATTO)"
        else
            log_fail "Latenza micro-append TROPPO ALTA: ${elapsed_lat_write}ms (Atteso <500ms)"
            all_passed=false
        fi

        # 4. Lettura Sequenziale Completa (Throughput)
        # Usiamo cat direzionato a null per forzare FUSE a leggere tutti i chunk
        local start_read=$(date +%s%3N)
        cat "$test_file" > /dev/null
        local end_read=$(date +%s%3N)
        local elapsed_read=$((end_read - start_read))
        
        local throughput_read=0
        if [ $elapsed_read -gt 0 ]; then
            throughput_read=$(( (size * 1000) / elapsed_read ))
        fi
        log_info "Lettura totale completata in ${elapsed_read}ms (~${throughput_read} MB/s)"

        # Pulizia per non esaurire lo spazio sul disco
        rm "$test_file"
        sleep 1 # Breve pausa per far stabilizzare il server tra un file e l'altro
    done
    
    if [ "$all_passed" = true ]; then
        log_pass "Tutti i test su file di grandi dimensioni completati con successo"
    else
        return 1
    fi
}

##############################################################################
# Test Suite 6: Caching
##############################################################################

test_cache_hit() {
    log_test "Cache Hit Verification"
    
    local test_file="$MOUNT_POINT/cache_test_${RANDOM}.txt"
    echo "cache test content" > "$test_file"
    
    # First read (cache miss)
    local content1=$(cat "$test_file")
    
    # Second read (cache hit)
    local content2=$(cat "$test_file")
    
    if [ "$content1" = "$content2" ]; then
        log_pass "Cache consistency verified"
    else
        log_fail "Cache inconsistency detected"
    fi
}

test_cache_invalidation_ttl() {
    log_test "Cache Invalidation (TTL)"
    
    local test_file="$MOUNT_POINT/ttl_test_${RANDOM}.txt"
    echo "original content" > "$test_file"
    
    # Read (cache)
    local content1=$(cat "$test_file")
    
    # Wait for TTL to expire (default 1 second)
    log_info "Waiting for TTL expiration..."
    sleep 2
    
    # Read again (should refresh)
    local content2=$(cat "$test_file")
    
    if [ "$content1" = "$content2" ]; then
        log_pass "Cache TTL handling verified"
    else
        log_fail "Cache TTL invalidation issue"
    fi
}

##############################################################################
# Test Suite 7: Error Handling
##############################################################################

test_read_nonexistent_file() {
    log_test "Read Non-existent File (Error Handling)"
    
    if ! cat "$MOUNT_POINT/nonexistent_${RANDOM}.txt" 2>/dev/null; then
        log_pass "Error handling for non-existent file works"
    else
        log_fail "Should fail on non-existent file"
    fi
}

test_delete_nonexistent_file() {
    log_test "Delete Non-existent File (Error Handling)"
    
    if ! rm "$MOUNT_POINT/nonexistent_${RANDOM}.txt" 2>/dev/null; then
        log_pass "Error handling for delete non-existent file works"
    else
        log_fail "Should fail on delete non-existent file"
    fi
}

##############################################################################
# Test Suite 8: Concurrent Operations
##############################################################################

test_concurrent_writes() {
    log_test "Concurrent Write Operations"
    
    local pids=()
    local prefix="${RANDOM}"
    
    # Start 5 concurrent writes
    for i in {1..5}; do
        {
            for j in {1..10}; do
                echo "Content $i-$j" > "$MOUNT_POINT/concurrent_${prefix}_$i.txt"
            done
        } &
        pids+=($!)
    done
    
    # Wait for all to complete
    local failed=0
    for pid in "${pids[@]}"; do
        if ! wait $pid 2>/dev/null; then
            ((failed++))
        fi
    done
    
    if [ $failed -eq 0 ]; then
        log_pass "Concurrent writes completed successfully"
    else
        log_fail "Some concurrent writes failed"
    fi
}

##############################################################################
# Test Suite 9: Special Operations
##############################################################################

test_file_rename() {
    log_test "File Rename Operation"
    
    local file1="$MOUNT_POINT/original_${RANDOM}.txt"
    local file2="$MOUNT_POINT/renamed_${RANDOM}.txt"
    
    echo "rename test" > "$file1"
    mv "$file1" "$file2"
    
    if [ ! -f "$file1" ] && [ -f "$file2" ]; then
        log_pass "File rename successful"
    else
        log_fail "File rename failed"
    fi
}

test_nested_directories() {
    log_test "Nested Directory Operations"
    
    local dir_id="${RANDOM}"
    local nested="$MOUNT_POINT/dir1_${dir_id}/dir2/dir3"
    mkdir -p "$nested" 2>/dev/null || true
    
    if [ -d "$nested" ]; then
        log_pass "Nested directory creation successful"
    else
        log_fail "Nested directory creation failed (expected behavior: may not be fully supported)"
    fi
}

##############################################################################
# Test Suite 10: Daemon Mode
##############################################################################

test_daemon_mode() {
    log_test "Background Execution (Daemon Mode)"
    
    local daemon_mount="/tmp/rfs-daemon-test"
    mkdir -p "$daemon_mount"
    
    # Avvia in modalità demone
    $RFS_BINARY --mount-point "$daemon_mount" --daemon > /dev/null 2>&1
    sleep 2
    
    if [ -f "/tmp/rfs.pid" ] && mountpoint -q "$daemon_mount"; then
        local d_pid=$(cat /tmp/rfs.pid)
        if kill -0 "$d_pid" 2>/dev/null; then
            log_pass "Demone avviato correttamente e sganciato dal terminale"
            kill "$d_pid"
            sleep 1
        else 
            log_fail "Il processo demone è morto prematuramente"
            return 1
        fi
    else
        log_fail "Avvio demone fallito o mountpoint non registrato"
        return 1
    fi
    
    # Pulizia del mount temporaneo
    if command -v fusermount3 >/dev/null 2>&1; then
        fusermount3 -u "$daemon_mount" 2>/dev/null || true
    else
        fusermount -u "$daemon_mount" 2>/dev/null || true
    fi
    rmdir "$daemon_mount" 2>/dev/null || true
}

##############################################################################
# Test Suite 11: Graceful Shutdown 
##############################################################################

test_graceful_shutdown() {
    log_test "Graceful Shutdown (Flush in chiusura)"
    
    local file_name="shutdown_${RANDOM}.txt"
    local test_file="$MOUNT_POINT/$file_name"
    
    echo "Dati salvati prima dello spegnimento" > "$test_file"
    
    log_info "Eseguo unmount pulito per innescare il flush..."
    if command -v fusermount3 >/dev/null 2>&1; then
        fusermount3 -u "$MOUNT_POINT"
    else
        fusermount -u "$MOUNT_POINT"
    fi
    sleep 2
    
    if [ "$(cat "$SERVER_STORAGE/$file_name" 2>/dev/null)" = "Dati salvati prima dello spegnimento" ]; then
        log_pass "I dati sono stati flushati al server prima della terminazione"
    else
        log_fail "Dati persi, il client non ha completato il flush in chiusura"
        return 1
    fi
    
    log_info "Riavvio FUSE daemon per i test rimanenti..."
    $RFS_BINARY --mount-point "$MOUNT_POINT" > /tmp/rfs_test_restart.log 2>&1 &
    RFS_PID=$!
    sleep 2
}

##############################################################################
# Test Suite 12: Network Resilience
##############################################################################

test_network_failure() {
    log_test "Network Resilience (Gestione caduta del Server)"
    
    # Uccidiamo brutalmente il server HTTP (kill -9)
    if [ ! -z "$RFS_SERVER_PID" ]; then
        kill -9 $RFS_SERVER_PID 2>/dev/null
        sleep 1
    fi
    
    # Proviamo a usare il file system con un timeout di 5 secondi
    timeout 5 ls "$MOUNT_POINT" > /dev/null 2>&1
    local exit_code=$?
    
    if [ $exit_code -eq 124 ]; then
        log_fail "Il client FUSE si è bloccato all'infinito (hang) senza il server"
        return 1
    elif [ $exit_code -ne 0 ]; then
        log_pass "Il client ha restituito un errore di I/O senza bloccarsi"
    else
        log_pass "Il client ha risposto istantaneamente usando la cache"
    fi
    
    # Svuotiamo la variabile così la funzione cleanup globale non restituisce errori
    RFS_SERVER_PID=""
}

##############################################################################
# Main Test Execution
##############################################################################

main() {
    echo -e "${BLUE}╔════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║  Remote File System - Test Suite                          ║${NC}"
    echo -e "${BLUE}║  Testing Against Specification                            ║${NC}"
    echo -e "${BLUE}╚════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    
    # Setup
    if [ "$SKIP_SETUP" = "1" ]; then
        log_info "SKIP_SETUP=1 - skipping setup and using existing mount at $MOUNT_POINT"
        if ! mountpoint -q "$MOUNT_POINT"; then
            log_fail "Mount point $MOUNT_POINT is not mounted. Aborting."
            return 1
        fi
    else
        setup
    fi
    echo ""
    
    # Test Suites
    echo -e "${YELLOW}=== SUITE 1: Mount/Unmount ===${NC}"
    test_mount_unmount
    echo ""
    
    echo -e "${YELLOW}=== SUITE 2: Basic CRUD Operations ===${NC}"
    test_create_file
    test_read_file
    test_update_file
    test_append_file
    test_delete_file
    test_mkdir
    test_rmdir
    test_list_directory
    echo ""
    
    echo -e "${YELLOW}=== SUITE 3: File Attributes ===${NC}"
    test_file_size
    test_file_permissions
    test_file_timestamps
    echo ""
    
    echo -e "${YELLOW}=== SUITE 4: Large Files ===${NC}"
    test_large_file_write
    test_large_file_read
    echo ""
    
    echo -e "${YELLOW}=== SUITE 5: Performance ===${NC}"
    test_strict_latency
    test_directory_listing_speed
    test_large_files_io_and_latency
    echo ""
    
    echo -e "${YELLOW}=== SUITE 6: Caching ===${NC}"
    test_cache_hit
    test_cache_invalidation_ttl
    echo ""
    
    echo -e "${YELLOW}=== SUITE 7: Error Handling ===${NC}"
    test_read_nonexistent_file
    test_delete_nonexistent_file
    echo ""
    
    echo -e "${YELLOW}=== SUITE 8: Concurrent Operations ===${NC}"
    test_concurrent_writes
    echo ""
    
    echo -e "${YELLOW}=== SUITE 9: Special Operations ===${NC}"
    test_file_rename
    test_nested_directories
    echo ""

    echo -e "${YELLOW}=== SUITE 10: Daemon Mode ===${NC}"
    test_daemon_mode
    echo ""
    
    echo -e "${YELLOW}=== SUITE 11: Graceful Shutdown ===${NC}"
    test_graceful_shutdown
    echo ""
    
    echo -e "${YELLOW}=== SUITE 12: Network Resilience ===${NC}"
    test_network_failure
    echo ""
    
    # Summary
    echo -e "${BLUE}╔════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║  Test Summary                                              ║${NC}"
    echo -e "${BLUE}╚════════════════════════════════════════════════════════════╝${NC}"
    echo -e "Total Tests: $((TEST_PASSED + TEST_RESULTS))"
    echo -e "${GREEN}Passed: $TEST_PASSED${NC}"
    echo -e "${RED}Failed: $TEST_RESULTS${NC}"
    
    if [ $TEST_RESULTS -eq 0 ]; then
        echo -e "${GREEN}✓ All tests passed!${NC}"
        return 0
    else
        echo -e "${RED}✗ Some tests failed.${NC}"
        return 1
    fi
}

# Run main
main
exit $?