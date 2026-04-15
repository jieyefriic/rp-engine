#!/bin/bash
# Universal test runner wrapper
# Usage: ./run_test.sh <yaml_path> <input_json> <output_path>
set -e

# Source API keys
source ~/.bashrc 2>/dev/null || true

# Run the test
cd /home/user/rp-engine
cargo run --example ut_runner -- "$1" "$2" "$3" 2>&1
