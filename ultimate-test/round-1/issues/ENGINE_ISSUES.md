# Engine Issues — Round 1

Issues found in the rp-engine itself during testing. Each entry describes a bug, limitation, or unexpected behavior in the engine code.

---

## EI-01: Zhipu and Moonshot providers hardcoded to DashScope endpoint — undocumented
- **Found in test**: [P07_R1], [P08_R1]
- **Source file**: `src/providers/zhipu.rs:14`, `src/providers/moonshot.rs:14`
- **Problem**: Both providers are hardcoded to `https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions`. Users providing native Zhipu (`open.bigmodel.cn`) or Moonshot (`api.moonshot.cn`) API keys will get authentication failures with no explanation.
- **Suggestion**: Document in skill.md that Zhipu/Moonshot require a DashScope API key, or allow `base_url` override.
- **Severity**: Medium

## EI-02: Huoshan structured output succeeds despite json:false — positive finding
- **Found in test**: [P10_R3]
- **Source file**: `src/providers/providers.json:180-182`, `src/nodes/generate.rs`
- **Problem**: Huoshan models marked `json: false` still work with output_schema via prompt-based JSON injection + `extract_json_from_content`.
- **Suggestion**: Document this graceful degradation in skill.md.
- **Severity**: Low (positive finding)

## EI-03: Huoshan ignores max_tokens setting
- **Found in test**: [P10_R3]
- **Source file**: `src/providers/huoshan.rs`, `src/providers/openai_compatible.rs`
- **Problem**: Huoshan test specified `max_tokens: 200` but returned 1105 output tokens. The max_tokens parameter appears to be ignored for this provider.
- **Expected behavior**: Output should be capped at approximately 200 tokens.
- **Actual behavior**: 1105 output tokens returned.
- **Suggestion**: Investigate whether the Huoshan (Volcano Engine) API supports max_tokens and if the engine is passing it correctly.
- **Severity**: Medium

## EI-04: PostgreSQL panics on JSONB/TEXT[]/UUID/TIMESTAMPTZ column deserialization
- **Found in test**: [D01]
- **Source file**: `src/nodes/connectors/postgresql.rs`
- **Problem**: Querying a table with JSONB, TEXT[], UUID, or TIMESTAMPTZ columns causes a panic (stack trace) rather than a graceful error. The error is "error deserializing column 5". Basic types (INT, TEXT, FLOAT, BOOL) work fine. Casting to `::text` also panics for JSONB.
- **Expected behavior**: JSONB → serde_json::Value, TEXT[] → array, UUID → string, TIMESTAMPTZ → string.
- **Actual behavior**: Unrecoverable panic.
- **Suggestion**: Add proper type handling in `row_value_to_json` for JSONB, array types, UUID, and TIMESTAMPTZ. Use `try_get` instead of `get` to avoid panics.
- **Severity**: Critical — panics crash the entire workflow instead of returning an error.

## EI-05: Node timeout (error_handling.timeout_ms) causes panic instead of graceful error
- **Found in test**: [N16]
- **Source file**: `src/engine/executor.rs`
- **Problem**: Setting `timeout_ms: 100` on a generate node causes a panic (stack trace) instead of returning `{_error: true, _error_message: "timeout"}`. The runner crashes before writing any result file.
- **Expected behavior**: With `on_exhausted: continue`, the node should produce `_error`/`_error_message` outputs and the workflow should continue.
- **Actual behavior**: Panic/crash.
- **Suggestion**: The timeout mechanism should use `tokio::time::timeout()` and catch the error gracefully. Ensure the timeout path goes through the same error handling as other failures.
- **Severity**: High — panics prevent error recovery workflows.

## EI-06: Qwen R5 tool calling returns empty output despite token consumption
- **Found in test**: [P06_R5]
- **Source file**: `src/nodes/generate.rs` (tool calling loop)
- **Problem**: Qwen (qwen-plus) tool calling consumed 2431 input + 1823 output tokens but the final `output` field was an empty string. The tool loop ran but produced no usable content.
- **Expected behavior**: After tool calls complete, the final LLM response should be captured as `output`.
- **Actual behavior**: Empty string output.
- **Suggestion**: Check if the final response after tool loop completion is properly extracted for Qwen provider. May be an issue with how the last response is captured when tool calls are present.
- **Severity**: Medium

## EI-07: output_schema fields null when response node maps individual schema fields
- **Found in test**: [N03], [N04]
- **Source file**: `src/nodes/generate.rs`, `src/state/execution_state.rs`
- **Problem**: When a generate node has `output_schema` with multiple fields (title, score, tags, etc.), and the response node maps them individually (`title: "gen.title"`, `score: "gen.score"`), all fields are null in the final output. The gen node appears to store its output as raw text under `output` instead of spreading the schema fields.
- **Expected behavior**: Schema fields should be individually addressable as `gen.title`, `gen.score`, etc.
- **Actual behavior**: All individual field paths return null. Only `gen.output` contains data (raw text).
- **Suggestion**: Either (a) the engine's JSON extraction needs to be more aggressive / fall back to re-parsing, or (b) skill.md should clarify that output_schema fields may not always be individually addressable, especially with providers that don't support native JSON mode.
- **Severity**: High — structured output is a core feature. This cascades to break multi-step workflows (S01, S02) that depend on structured fields from upstream nodes.

## EI-08: Object variables render as `[object]` in Handlebars templates
- **Found in test**: [S16]
- **Source file**: `src/nodes/generate.rs` (template rendering)
- **Problem**: When a variable bound from a transform node contains a JSON object/map, the Handlebars template renders it as `[object]` or `[object Object]` instead of a JSON string. In S16, the merge transform produced `{openai: "...", anthropic: "...", qwen: "..."}` but the synthesize template received `[object]` for `{{answers}}`.
- **Expected behavior**: Object variables should be serialized to JSON string in the template.
- **Actual behavior**: Renders as `[object]`.
- **Suggestion**: Before Handlebars rendering, serialize non-string/non-number variable values with `serde_json::to_string()`. Or register a custom Handlebars helper for JSON serialization.
- **Severity**: Medium — affects any workflow that passes structured data between nodes via templates.

## EI-09: on_error edge output not captured by response node
- **Found in test**: [N52], [S17]
- **Source file**: `src/engine/executor.rs` (edge evaluation + dead path propagation)
- **Problem**: When a node fails (with `on_exhausted: continue`) and the workflow routes via an `on_error: true` edge to a fallback node, the fallback executes correctly but the response node outputs null for the fallback fields. The fallback node's output exists in state but the response node can't read it.
- **Expected behavior**: Response node should be able to read outputs from any executed node, including those reached via on_error edges.
- **Actual behavior**: `answer: null` despite fallback node producing valid content.
- **Suggestion**: Investigate whether on_error routing affects the response node's ability to read state. May be related to dead path detection incorrectly marking the fallback→response edge.
- **Severity**: High — error recovery workflows are non-functional if fallback output can't be consumed.

## EI-10: Redis connector only supports 7 operations, missing hash/list/sorted set
- **Found in test**: [D14], [D16]
- **Source file**: `src/nodes/connectors/redis.rs:327-338`
- **Problem**: The Redis connector only supports: `get`, `set`, `del`, `publish`, `json_append`, `json_merge`, `json_delete`. Common Redis operations like `hset`, `hgetall`, `lpush`, `lrange`, `sadd`, `smembers`, `zadd`, `zrange` are all unsupported and return `InvalidOperation` error.
- **Expected behavior**: At minimum, hash (`hset`/`hgetall`) and list (`lpush`/`lrange`) operations should be supported, as they are among the most common Redis usage patterns.
- **Actual behavior**: `Unknown Redis operation: hset` error.
- **Suggestion**: Add support for at least: `hset`, `hget`, `hgetall`, `hdel`, `lpush`, `rpush`, `lrange`, `lpop`, `rpop`. Optionally: sorted sets (`zadd`, `zrange`), sets (`sadd`, `smembers`).
- **Severity**: Medium — limits Redis to simple key-value and JSON operations only.
