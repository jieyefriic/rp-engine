#!/bin/bash
# Batch test runner for ultimate-test
# Usage: ./run_batch.sh <workflow_dir_pattern> <input_json>
# Example: ./run_batch.sh "dim1-providers/p*_r1_*" '{"query": "What is 2+2?"}'
set -e

# Source API keys
export OPENAI_API_KEY="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"
export ANTHROPIC_API_KEY="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"
export GEMINI_API_KEY="AIzaSyCpOKbcbrs-iVWlMRE2Tse6opiFCromGRU"
export XAI_API_KEY="xai-ZMlIeWMvr4tn0YNT02jyBhnS9iBEht6LHNhgWMWNT3oEzInFxso4BbH585ug81ujyhLdqDOWjT5Wl1qz"
export DEEPSEEK_API_KEY="sk-5d3bc96906294872ac9b70d38a391bc9"
export MINIMAX_API_KEY="sk-api-AVsKZb46ImEOcllLu8PNcWXVkr4sZG57GoxplvIE1NCRbgis6W1aivZR2JgnF4ycM73pCwDnduINy8sAtLGqluLz_e6AKRMKqEjf0DqlyJszPfRuGcGX0g4"
export QWEN_API_KEY="sk-18c59231d9f1452ab4f91ab81dd063f0"
export HUOSHAN_API_KEY="db1d143a-cb48-48e5-8560-d110ccc83fe8"
# Zhipu and Moonshot now use native API endpoints (no longer via DashScope)
export MOONSHOT_API_KEY="sk-pEzyGBzldvkkYQlvRbSrgLDu29GqqstnrK343VGDawCpQzYm"
export ZHIPU_API_KEY="593e20ffd5ba446a9ac897d069f9b30b.7YvcIDHui89XxOh8"

cd /home/user/rp-engine
BASE="ultimate-test/round-1"
PATTERN="$1"
INPUT="$2"

PASS=0
FAIL=0
TOTAL=0

for yaml_file in $BASE/workflows/$PATTERN; do
    [ -f "$yaml_file" ] || continue
    TOTAL=$((TOTAL + 1))

    # Derive output path
    basename=$(basename "$yaml_file" .yaml)
    dim_dir=$(basename $(dirname "$yaml_file"))
    output_file="$BASE/results/$dim_dir/${basename}.json"
    mkdir -p "$(dirname "$output_file")"

    echo "================================================"
    echo "TEST $TOTAL: $basename"
    echo "================================================"

    if timeout 120 cargo run --example ut_runner -- "$yaml_file" "$INPUT" "$output_file" 2>&1; then
        PASS=$((PASS + 1))
        echo ">>> PASS"
    else
        FAIL=$((FAIL + 1))
        echo ">>> FAIL"
    fi
    echo ""
done

echo "================================================"
echo "SUMMARY: $PASS/$TOTAL passed, $FAIL failed"
echo "================================================"
