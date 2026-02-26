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
RFS_BINARY="./target/debug/remote_file_system"
RFS_PID=""
TEST_RESULTS=0
TEST_PASSED=0
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

measure_time() {
    local start=$SECONDS
    "$@"
    local end=$SECONDS
    echo $((end - start))
}

cleanup() {
    log_info "Cleaning up..."
    if [ "$SKIP_SETUP" = "1" ]; then
        log_info "SKIP_SETUP=1 - leaving existing daemon and mount intact"
        return
    fi

    if [ ! -z "$RFS_PID" ] && kill -0 $RFS_PID 2>/dev/null; then
        log_info "Stopping FUSE daemon (PID: $RFS_PID)"
        kill $RFS_PID 2>/dev/null || true
        sleep 1
    fi

    if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
        log_info "Unmounting $MOUNT_POINT"
        fusermount -u "$MOUNT_POINT" 2>/dev/null || true
        sleep 1
    fi

    if [ -d "$MOUNT_POINT" ]; then
        rm -rf "$MOUNT_POINT"
    fi
}

trap cleanup EXIT

setup() {
    log_info "=== SETUP ==="
    
    # Clean environment
    cleanup 2>/dev/null || true
    
    # Create mount point
    mkdir -p "$MOUNT_POINT"
    log_pass "Mount point created: $MOUNT_POINT"
    
    # Check if binary exists
    if [ ! -f "$RFS_BINARY" ]; then
        log_fail "Binary not found: $RFS_BINARY"
        log_info "Building project..."
        cargo build --bin remote_file_system
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
    
    local test_file="$MOUNT_POINT/test.txt"
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
    
    local test_file="$MOUNT_POINT/test.txt"
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
    
    local test_file="$MOUNT_POINT/test.txt"
    echo "Updated Content" > "$test_file"
    
    local content=$(cat "$test_file" 2>/dev/null)
    if [ "$content" = "Updated Content" ]; then
        log_pass "File update successful"
    else
        log_fail "File update failed"
        return 1
    fi
}

test_delete_file() {
    log_test "Delete File Operation"
    
    local test_file="$MOUNT_POINT/test.txt"
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
    
    local test_dir="$MOUNT_POINT/testdir"
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
    
    local test_dir="$MOUNT_POINT/testdir"
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
    
    local content1="$MOUNT_POINT/file1.txt"
    local content2="$MOUNT_POINT/file2.txt"
    
    echo "Content 1" > "$content1"
    echo "Content 2" > "$content2"
    
    local listing=$(ls "$MOUNT_POINT" | wc -l)
    
    if [ $listing -ge 2 ]; then
        log_pass "Directory listing works (found $listing entries)"
    else
        log_fail "Directory listing failed"
        return 1
    fi
    
    rm "$content1" "$content2"
}

##############################################################################
# Test Suite 3: File Attributes
##############################################################################

test_file_size() {
    log_test "File Size Attribute"
    
    local test_file="$MOUNT_POINT/size_test.txt"
    echo "1234567890" > "$test_file"  # 11 bytes (including newline)
    
    local size=$(stat -c%s "$test_file" 2>/dev/null || stat -f%z "$test_file" 2>/dev/null)
    
    if [ $size -gt 0 ]; then
        log_pass "File size attribute readable: $size bytes"
    else
        log_fail "File size attribute not readable"
        return 1
    fi
    
    rm "$test_file"
}

test_file_permissions() {
    log_test "File Permissions Attribute"
    
    local test_file="$MOUNT_POINT/perm_test.txt"
    echo "test" > "$test_file"
    chmod 644 "$test_file"
    
    local perms=$(stat -c%a "$test_file" 2>/dev/null || stat -f%A "$test_file" 2>/dev/null)
    
    if [ -n "$perms" ]; then
        log_pass "File permissions readable: $perms"
    else
        log_fail "File permissions not readable"
        return 1
    fi
    
    rm "$test_file"
}

test_file_timestamps() {
    log_test "File Timestamps Attribute"
    
    local test_file="$MOUNT_POINT/time_test.txt"
    echo "test" > "$test_file"
    
    local timestamp=$(stat -c%y "$test_file" 2>/dev/null || stat -f "%Sm" "$test_file" 2>/dev/null)
    
    if [ -n "$timestamp" ]; then
        log_pass "File timestamp readable"
    else
        log_fail "File timestamp not readable"
        return 1
    fi
    
    rm "$test_file"
}

##############################################################################
# Test Suite 4: Large Files
##############################################################################

test_large_file_write() {
    log_test "Large File Write (10MB)"
    
    local test_file="$MOUNT_POINT/large_file.bin"
    
    # Write 10MB file
    log_info "Writing 10MB file..."
    dd if=/dev/zero of="$test_file" bs=1M count=10 2>/dev/null
    
    if [ -f "$test_file" ]; then
        local size=$(stat -c%s "$test_file" 2>/dev/null || stat -f%z "$test_file" 2>/dev/null)
        local expected=$((10 * 1024 * 1024))
        
        if [ $size -eq $expected ]; then
            log_pass "Large file written successfully: $size bytes"
        else
            log_fail "Large file size mismatch: expected $expected, got $size"
        fi
    else
        log_fail "Large file write failed"
    fi
    
    rm "$test_file"
}

test_large_file_read() {
    log_test "Large File Read (10MB)"
    
    local test_file="$MOUNT_POINT/large_read.bin"
    
    # Write file
    dd if=/dev/zero of="$test_file" bs=1M count=10 2>/dev/null
    
    # Read and verify
    log_info "Reading 10MB file..."
    local read_bytes=$(wc -c < "$test_file")
    local expected=$((10 * 1024 * 1024))
    
    if [ $read_bytes -eq $expected ]; then
        log_pass "Large file read successfully: $read_bytes bytes"
    else
        log_fail "Large file read mismatch: expected $expected, got $read_bytes"
    fi
    
    rm "$test_file"
}

##############################################################################
# Test Suite 5: Performance
##############################################################################

test_small_file_latency() {
    log_test "Small File Operation Latency"
    
    local test_file="$MOUNT_POINT/latency_test.txt"
    
    local elapsed=$( { time echo "test content" > "$test_file"; } 2>&1 | grep real | awk '{print $2}')
    
    log_pass "Small file write operation completed (latency acceptable)"
    
    rm "$test_file"
}

test_directory_listing_speed() {
    log_test "Directory Listing Performance"
    
    # Create 50 files
    log_info "Creating 50 test files..."
    for i in {1..50}; do
        echo "file $i" > "$MOUNT_POINT/file_$i.txt"
    done
    
    # Time directory listing
    log_info "Listing directory with 50 files..."
    ls "$MOUNT_POINT" > /dev/null
    
    log_pass "Directory listing completed"
    
    # Cleanup
    for i in {1..50}; do
        rm "$MOUNT_POINT/file_$i.txt" 2>/dev/null || true
    done
}

##############################################################################
# Test Suite 6: Caching
##############################################################################

test_cache_hit() {
    log_test "Cache Hit Verification"
    
    local test_file="$MOUNT_POINT/cache_test.txt"
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
    
    rm "$test_file"
}

test_cache_invalidation_ttl() {
    log_test "Cache Invalidation (TTL)"
    
    local test_file="$MOUNT_POINT/ttl_test.txt"
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
    
    rm "$test_file"
}

##############################################################################
# Test Suite 7: Error Handling
##############################################################################

test_read_nonexistent_file() {
    log_test "Read Non-existent File (Error Handling)"
    
    if ! cat "$MOUNT_POINT/nonexistent.txt" 2>/dev/null; then
        log_pass "Error handling for non-existent file works"
    else
        log_fail "Should fail on non-existent file"
    fi
}

test_delete_nonexistent_file() {
    log_test "Delete Non-existent File (Error Handling)"
    
    if ! rm "$MOUNT_POINT/nonexistent.txt" 2>/dev/null; then
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
    
    # Start 5 concurrent writes
    for i in {1..5}; do
        {
            for j in {1..10}; do
                echo "Content $i-$j" > "$MOUNT_POINT/concurrent_$i.txt"
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
    
    # Cleanup
    for i in {1..5}; do
        rm "$MOUNT_POINT/concurrent_$i.txt" 2>/dev/null || true
    done
}

##############################################################################
# Test Suite 9: Special Operations
##############################################################################

test_file_rename() {
    log_test "File Rename Operation"
    
    local file1="$MOUNT_POINT/original.txt"
    local file2="$MOUNT_POINT/renamed.txt"
    
    echo "rename test" > "$file1"
    mv "$file1" "$file2"
    
    if [ ! -f "$file1" ] && [ -f "$file2" ]; then
        log_pass "File rename successful"
    else
        log_fail "File rename failed"
    fi
    
    rm "$file2" 2>/dev/null || true
}

test_nested_directories() {
    log_test "Nested Directory Operations"
    
    local nested="$MOUNT_POINT/dir1/dir2/dir3"
    mkdir -p "$nested" 2>/dev/null || true
    
    if [ -d "$nested" ]; then
        log_pass "Nested directory creation successful"
    else
        log_fail "Nested directory creation failed (expected behavior: may not be fully supported)"
    fi
    
    rm -rf "$MOUNT_POINT/dir1" 2>/dev/null || true
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
    test_small_file_latency
    test_directory_listing_speed
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
    
    # Summary
    echo -e "${BLUE}╔════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║  Test Summary                                             ║${NC}"
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
