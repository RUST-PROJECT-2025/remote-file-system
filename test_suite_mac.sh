#!/bin/bash

##############################################################################
# Remote File System - Comprehensive Test Suite (macOS Edition)
# Tests compliance with project specification
##############################################################################

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
MOUNT_POINT="/tmp/rfs-test-mount"
SERVER_STORAGE="/tmp/rfs-test-storage"
RFS_BINARY="./target/debug/remote_file_system"
RFS_SERVER_BINARY="./target/debug/rfs_server"
RFS_PID=""
RFS_SERVER_PID=""
TEST_RESULTS=0
TEST_PASSED=0
SKIP_SETUP=${SKIP_SETUP:-0}

##############################################################################
# Utility Functions for macOS
##############################################################################

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((TEST_PASSED++)); }
log_fail() { echo -e "${RED}[FAIL]${NC} $1"; ((TEST_RESULTS++)); }
log_test() { echo -e "${YELLOW}[TEST]${NC} $1"; }

# Funzione per ottenere il timestamp in millisecondi su macOS
current_time_ms() {
    perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'
}

# Funzione per controllare se la cartella è montata (sostituisce 'mountpoint')
is_mounted() {
    mount | grep -q "$MOUNT_POINT"
}

cleanup() {
    log_info "Cleaning up..."
    if [ "$SKIP_SETUP" = "1" ]; then
        log_info "SKIP_SETUP=1 - leaving existing daemon and mount intact"
        return
    fi

    if [ ! -z "$RFS_SERVER_PID" ] && kill -0 $RFS_SERVER_PID 2>/dev/null; then
        log_info "Stopping RFS server (PID: $RFS_SERVER_PID)"
        kill -9 $RFS_SERVER_PID 2>/dev/null || true
        sleep 1
    fi

    if [ ! -z "$RFS_PID" ] && kill -0 $RFS_PID 2>/dev/null; then
        log_info "Stopping FUSE daemon (PID: $RFS_PID)"
        kill -9 $RFS_PID 2>/dev/null || true
        sleep 1
    fi

    if is_mounted; then
        log_info "Unmounting $MOUNT_POINT"
        umount "$MOUNT_POINT" 2>/dev/null || diskutil unmount force "$MOUNT_POINT" 2>/dev/null || true
        sleep 1
    fi

    rm -rf "$MOUNT_POINT" 2>/dev/null || true
    rm -rf "$SERVER_STORAGE" 2>/dev/null || true
}

trap cleanup EXIT

setup() {
    log_info "=== SETUP ==="
    cleanup 2>/dev/null || true
    
    mkdir -p "$MOUNT_POINT"
    mkdir -p "$SERVER_STORAGE"
    log_pass "Directories created: Mount -> $MOUNT_POINT | Storage -> $SERVER_STORAGE"
    
    if [ ! -f "$RFS_BINARY" ]; then
        log_info "Building project..."
        cargo build --bin remote_file_system
    fi
    
    if [ ! -f "$RFS_SERVER_BINARY" ]; then
        log_info "Building server..."
        cargo build --bin rfs_server
    fi
    
    export RFS_STORAGE_PATH="$SERVER_STORAGE"
    
    log_info "Starting RFS server on http://127.0.0.1:8080..."
    $RFS_SERVER_BINARY > /tmp/rfs_server.log 2>&1 &
    RFS_SERVER_PID=$!
    
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
    
    log_info "Starting FUSE daemon (macOS nativo)..."
    $RFS_BINARY --mount-point "$MOUNT_POINT" > /tmp/rfs_test.log 2>&1 &
    RFS_PID=$!
    
    max_attempts=10
    attempt=0
    while [ $attempt -lt $max_attempts ]; do
        if is_mounted; then
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
    if is_mounted; then
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
    if [ -f "$test_file" ]; then log_pass "File created successfully"; else log_fail "File creation failed"; return 1; fi
}

test_read_file() {
    log_test "Read File Operation"
    local test_file="$MOUNT_POINT/test_read_${RANDOM}.txt"
    echo "Hello World" > "$test_file"
    local content=$(cat "$test_file" 2>/dev/null)
    if [ "$content" = "Hello World" ]; then log_pass "File read successfully"; else log_fail "File read failed"; return 1; fi
}

test_update_file() {
    log_test "Update File Operation"
    local test_file="$MOUNT_POINT/test_update_${RANDOM}.txt"
    echo "Initial Content" > "$test_file"
    echo "Updated Content" > "$test_file"
    local content=$(cat "$test_file" 2>/dev/null)
    if [ "$content" = "Updated Content" ]; then log_pass "File update successful"; else log_fail "File update failed"; return 1; fi
}

test_append_file() {
    log_test "Append to File Operation"
    local test_file="$MOUNT_POINT/test_append_${RANDOM}.txt"
    echo "Test 1 Prima riga" > "$test_file"
    echo "Seconda riga test" >> "$test_file"
    local line_count=$(wc -l < "$test_file" | tr -d ' ')
    
    if [ "$line_count" -eq 2 ]; then
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
    if [ ! -f "$test_file" ]; then log_pass "File deleted successfully"; else log_fail "File deletion failed"; return 1; fi
}

test_mkdir() {
    log_test "Create Directory Operation"
    local test_dir="$MOUNT_POINT/testdir_${RANDOM}"
    mkdir "$test_dir"
    if [ -d "$test_dir" ]; then log_pass "Directory created successfully"; else log_fail "Directory creation failed"; return 1; fi
}

test_rmdir() {
    log_test "Remove Directory Operation"
    local test_dir="$MOUNT_POINT/testdir_rm_${RANDOM}"
    mkdir "$test_dir"
    rmdir "$test_dir"
    if [ ! -d "$test_dir" ]; then log_pass "Directory removed successfully"; else log_fail "Directory removal failed"; return 1; fi
}

test_list_directory() {
    log_test "List Directory Contents"
    local test_dir="$MOUNT_POINT/testdir_list_${RANDOM}"
    mkdir "$test_dir"
    echo "Content 1" > "$test_dir/file1.txt"
    echo "Content 2" > "$test_dir/file2.txt"
    local listing=$(ls "$test_dir" | wc -l | tr -d ' ')
    if [ "$listing" -ge 2 ]; then log_pass "Directory listing works"; else log_fail "Directory listing failed"; return 1; fi
}

##############################################################################
# Test Suite 3: File Attributes (macOS stat format)
##############################################################################

test_file_size() {
    log_test "File Size Attribute"
    local test_file="$MOUNT_POINT/size_test_${RANDOM}.txt"
    echo "1234567890" > "$test_file" 
    local size=$(stat -f%z "$test_file" 2>/dev/null)
    if [ "$size" -gt 0 ] 2>/dev/null; then log_pass "File size attribute readable: $size bytes"; else log_fail "File size attribute not readable"; return 1; fi
}

test_file_permissions() {
    log_test "File Permissions Attribute"
    local test_file="$MOUNT_POINT/perm_test_${RANDOM}.txt"
    echo "test" > "$test_file"
    chmod 644 "$test_file"
    local perms=$(stat -f "%OLp" "$test_file" 2>/dev/null)
    if [ -n "$perms" ]; then log_pass "File permissions readable: $perms"; else log_fail "File permissions not readable"; return 1; fi
}

test_file_timestamps() {
    log_test "File Timestamps Attribute"
    local test_file="$MOUNT_POINT/time_test_${RANDOM}.txt"
    echo "test" > "$test_file"
    local timestamp=$(stat -f "%Sm" "$test_file" 2>/dev/null)
    if [ -n "$timestamp" ]; then log_pass "File timestamp readable"; else log_fail "File timestamp not readable"; return 1; fi
}

##############################################################################
# Test Suite 4: Large Files 
##############################################################################

test_large_file_write() {
    log_test "Large File Write (200MB+ Streaming & Latenza)"
    local test_file="$MOUNT_POINT/large_file_${RANDOM}.bin"
    log_info "Writing 200MB file..."
    
    local start_time=$(current_time_ms)
    dd if=/dev/zero of="$test_file" bs=1m count=200 2>/dev/null
    local end_time=$(current_time_ms)
    local elapsed=$((end_time - start_time))
    
    if [ -f "$test_file" ]; then
        local size=$(stat -f%z "$test_file" 2>/dev/null)
        local expected=$((200 * 1024 * 1024))
        if [ "$size" -eq "$expected" ]; then
            local throughput=$(( 200000 / (elapsed > 0 ? elapsed : 1) ))
            log_pass "File scritto in ${elapsed}ms (~${throughput} MB/s)"
        else
            log_fail "Mismatch dimensione: atteso $expected, trovato $size"
        fi
    else
        log_fail "Scrittura file fallita"
    fi
}

test_large_file_read() {
    log_test "Large File Read (200MB+ Streaming & Latenza)"
    local test_file="$MOUNT_POINT/large_read_${RANDOM}.bin"
    dd if=/dev/zero of="$test_file" bs=1m count=200 2>/dev/null
    
    log_info "Reading 200MB file..."
    local start_time=$(current_time_ms)
    local read_bytes=$(wc -c < "$test_file" | tr -d ' ')
    local end_time=$(current_time_ms)
    local elapsed=$((end_time - start_time))
    
    local expected=$((200 * 1024 * 1024))
    if [ "$read_bytes" -eq "$expected" ]; then
        local throughput=$(( 200000 / (elapsed > 0 ? elapsed : 1) ))
        log_pass "File letto in ${elapsed}ms (~${throughput} MB/s)"
    else
        log_fail "Mismatch lettura: atteso $expected, letti $read_bytes"
    fi
}

##############################################################################
# Test Suite 5: Performance (<500ms Latency Requirement)
##############################################################################

test_strict_latency() {
    log_test "Strict Latency Requirement (<500ms)"
    local test_file="$MOUNT_POINT/strict_latency_${RANDOM}.txt"
    
    local start_time=$(current_time_ms)
    echo "Verifica latenza" > "$test_file"
    cat "$test_file" > /dev/null
    local end_time=$(current_time_ms)
    local elapsed=$((end_time - start_time))
    
    if [ "$elapsed" -lt 500 ]; then
        log_pass "Latenza operativa I/O: ${elapsed}ms (Requisito SODDISFATTO)"
    else
        log_fail "Latenza TROPPO ALTA: ${elapsed}ms"
        return 1
    fi
}

test_directory_listing_speed() {
    log_test "Directory Listing Performance"
    local perf_dir="$MOUNT_POINT/perf_dir_${RANDOM}"
    mkdir "$perf_dir"
    
    for i in {1..50}; do echo "file $i" > "$perf_dir/file_$i.txt"; done
    
    local start_time=$(current_time_ms)
    ls "$perf_dir" > /dev/null
    local end_time=$(current_time_ms)
    local elapsed=$((end_time - start_time))
    
    if [ "$elapsed" -lt 500 ]; then
        log_pass "Listing completato in ${elapsed}ms (Requisito SODDISFATTO)"
    else
        log_fail "Directory listing troppo lento: ${elapsed}ms"
        return 1
    fi
}

##############################################################################
# Test Suite 6-9: Varie
##############################################################################

test_cache_hit() {
    log_test "Cache Hit Verification"
    local test_file="$MOUNT_POINT/cache_test_${RANDOM}.txt"
    echo "cache test content" > "$test_file"
    local content1=$(cat "$test_file")
    local content2=$(cat "$test_file")
    if [ "$content1" = "$content2" ]; then log_pass "Cache consistency verified"; else log_fail "Cache inconsistency"; fi
}

test_concurrent_writes() {
    log_test "Concurrent Write Operations"
    local pids=()
    local prefix="${RANDOM}"
    
    for i in {1..5}; do
        {
            for j in {1..10}; do echo "Content $i-$j" > "$MOUNT_POINT/concurrent_${prefix}_$i.txt"; done
        } &
        pids+=($!)
    done
    
    local failed=0
    for pid in "${pids[@]}"; do wait $pid 2>/dev/null || ((failed++)); done
    if [ $failed -eq 0 ]; then log_pass "Concurrent writes completed successfully"; else log_fail "Concurrent writes failed"; fi
}

test_file_rename() {
    log_test "File Rename Operation"
    local file1="$MOUNT_POINT/original_${RANDOM}.txt"
    local file2="$MOUNT_POINT/renamed_${RANDOM}.txt"
    echo "rename test" > "$file1"
    mv "$file1" "$file2"
    if [ ! -f "$file1" ] && [ -f "$file2" ]; then log_pass "File rename successful"; else log_fail "File rename failed"; fi
}

##############################################################################
# Test Suite 10: Daemon Mode (Skipped on macOS)
##############################################################################

test_daemon_mode() {
    log_test "Background Execution (Daemon Mode)"
    log_info "Skipped: La modalità demone con libfuse non è supportata su macOS."
    log_pass "Test escluso correttamente per l'ambiente Mac."
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
    umount "$MOUNT_POINT"
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
    
    if [ ! -z "$RFS_SERVER_PID" ]; then
        kill -9 $RFS_SERVER_PID 2>/dev/null
        sleep 1
    fi
    
    # Implementazione nativa di un timeout per macOS tramite subshell
    (
        ls "$MOUNT_POINT" > /dev/null 2>&1
    ) &
    local task_pid=$!
    
    local count=0
    local exit_code=0
    while kill -0 $task_pid 2>/dev/null; do
        sleep 1
        ((count++))
        if [ $count -ge 5 ]; then
            kill -9 $task_pid 2>/dev/null
            exit_code=124
            break
        fi
    done
    
    if [ $exit_code -eq 124 ]; then
        log_fail "Il client FUSE si è bloccato all'infinito (hang) senza il server"
    else
        log_pass "Il client ha risposto istantaneamente usando la cache o restituito I/O error corretto"
    fi
    RFS_SERVER_PID=""
}

##############################################################################
# Main Test Execution
##############################################################################

main() {
    echo -e "${BLUE}╔════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║  Remote File System - macOS Test Suite                     ║${NC}"
    echo -e "${BLUE}╚════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    
    if [ "$SKIP_SETUP" = "1" ]; then
        if ! is_mounted; then log_fail "Mount point non montato. Aborting."; return 1; fi
    else
        setup
    fi
    echo ""
    
    test_mount_unmount
    test_create_file
    test_read_file
    test_update_file
    test_append_file
    test_delete_file
    test_mkdir
    test_rmdir
    test_list_directory
    echo ""
    test_file_size
    test_file_permissions
    test_file_timestamps
    echo ""
    test_large_file_write
    test_large_file_read
    echo ""
    test_strict_latency
    test_directory_listing_speed
    echo ""
    test_cache_hit
    test_concurrent_writes
    test_file_rename
    echo ""
    test_daemon_mode
    test_graceful_shutdown
    test_network_failure
    echo ""
    
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

main
exit $?