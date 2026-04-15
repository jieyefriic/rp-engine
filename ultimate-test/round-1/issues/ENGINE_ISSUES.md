# Engine Issues — Round 1

Issues found in the rp-engine during testing. Entries marked [FIXED] have code fixes applied (commit 065f951). Entries marked [PARTIAL] or [OPEN] still need work.

---

## EI-01: Zhipu and Moonshot providers hardcoded to DashScope endpoint [FIXED — docs]
- **Found in test**: [P07_R1], [P08_R1]
- **Source file**: `src/providers/zhipu.rs:14`, `src/providers/moonshot.rs:14`
- **Problem**: Both providers hardcoded to DashScope URL. Native API keys fail silently.
- **Fix applied**: Added documentation note to `docs/skill.md` in Providers section.
- **Severity**: Medium

## EI-02: Huoshan structured output succeeds despite json:false [INFO — no fix needed]
- **Found in test**: [P10_R3]
- **Problem**: Huoshan models (json: false) still work with output_schema via prompt + extraction.
- **Fix applied**: Documented graceful degradation in skill.md.
- **Severity**: Low (positive finding)

## EI-03: Huoshan ignores max_tokens setting [FIXED — docs]
- **Found in test**: [P10_R3]
- **Problem**: Huoshan `max_tokens: 200` returned 1105 tokens. Caused by thinking model tokens counted separately.
- **Fix applied**: Documented as expected thinking model behavior in skill.md.
- **Severity**: Medium

## EI-04: PostgreSQL panics on JSONB/TEXT[]/UUID/TIMESTAMPTZ [FIXED — verified]
- **Found in test**: [D01]
- **Source file**: `src/nodes/connectors/postgresql.rs:772-831`
- **Problem**: `row.get()` panics on type mismatch. No array type support.
- **Fix applied**: Replaced all `row.get()` with `row.try_get()`. Added 9 array type handlers (BOOL_ARRAY, INT2/4/8_ARRAY, FLOAT4/8_ARRAY, TEXT/VARCHAR_ARRAY, UUID_ARRAY, JSON/JSONB_ARRAY). Fallback returns `Value::Null` instead of panic.
- **Verification**: D01 PG SELECT with complex types now PASS.
- **Severity**: Critical → Resolved

## EI-05: Node timeout wraps already-resolved future [FIXED — verified]
- **Found in test**: [N16]
- **Source file**: `src/engine/executor.rs:1053-1068`
- **Problem**: `tokio::time::timeout` wrapped an already-awaited result (no-op). Timeout never fires.
- **Fix applied**: Moved timeout to wrap the actual `execute_node_once()` call.
- **Verification**: N16 with `timeout_ms: 100` now returns `{_error: true}` in 104ms.
- **Severity**: High → Resolved

## EI-06: Qwen empty output after tool calling loop exhaustion [FIXED — unverified]
- **Found in test**: [P06_R5]
- **Source file**: `src/nodes/generate.rs:629-706`
- **Problem**: If LLM keeps requesting tool calls until max_rounds, `final_content` stays empty.
- **Fix applied**: Track `last_assistant_content` during loop; use as fallback when loop exhausts.
- **Verification**: Not re-tested (needs Qwen tool calling regression test).
- **Severity**: Medium

## EI-07: output_schema fields null on JSON parse failure [FIXED — unverified]
- **Found in test**: [N03], [N04], [S01], [S02], [S35], [S39]
- **Source file**: `src/nodes/generate.rs:755-763`
- **Problem**: When JSON parsing fails, only `output` (raw text) stored. Schema fields never written.
- **Fix applied**: In the `Err` branch, now populates schema fields with typed defaults (`""` for string, `0` for number, etc.) alongside `output`.
- **Verification**: Not re-tested (needs LLM regression test with DeepSeek).
- **Severity**: High

## EI-08: Object variables render as `[object]` in Handlebars [FIXED — verified]
- **Found in test**: [S16]
- **Source file**: `src/nodes/generate.rs:972-976`
- **Problem**: Handlebars renders Object/Array values as `[object Object]`.
- **Fix applied**: Pre-serialize Object/Array values to JSON strings before Handlebars rendering.
- **Verification**: S16 `{{answers}}` now renders proper JSON.
- **Severity**: Medium → Resolved

## EI-09: on_error edge output not captured by response node [PARTIAL — still broken]
- **Found in test**: [N52], [S17]
- **Source file**: `src/engine/executor.rs:1362-1367`
- **Problem**: Fallback node via on_error edge executes, but response reads null for fallback fields.
- **Fix applied**: Changed `propagate_dead_paths()` to consider traversed edges (edge_count > 0) as active.
- **Verification**: **STILL FAILING** — N52 and S17 fallback output still null in response. The dead path fix is correct but the root cause is deeper: likely an execution ordering issue where the response node's input dependency on the failed node is resolved before the fallback completes, OR the response node reads state before the fallback writes.
- **Remaining work**: Need to investigate executor's node scheduling and state write timing.
- **Severity**: High — remains open

## EI-10: Redis connector missing hash/list operations [FIXED — verified]
- **Found in test**: [D14], [D16]
- **Source file**: `src/nodes/connectors/redis.rs:327-339`
- **Problem**: Only 7 operations supported. No hash or list support.
- **Fix applied**: Added 10 new operations: `hset`, `hget`, `hgetall`, `hdel`, `lpush`, `rpush`, `lpop`, `rpop`, `lrange`, `llen`.
- **Verification**: D14 Redis HSET/HGETALL now PASS.
- **Severity**: Medium → Resolved

---

## Post-Fix Issues (discovered during verification)

## EI-11: Checkpoint resume does not execute downstream nodes [OPEN]
- **Found in test**: Advanced checkpoint_resume test
- **Source file**: `src/engine/executor.rs` (resume logic) and `src/engine/engine.rs:252-314`
- **Problem**: After `engine.resume()` with `resume_from: "gen_evaluate"`, the status is Success but `nodes_executed` only contains `[pause, gen_question, start]` — the downstream nodes (`gen_evaluate`, `response`) never run. The `output` is empty `{}`.
- **Expected behavior**: Resume should re-execute from the specified node and all downstream.
- **Actual behavior**: Resume returns Success immediately without executing downstream.
- **Suggestion**: Investigate the `ResumeParams` handling in `execute_workflow()`. The forward-reachable set computation from the resume point may not be including the right nodes.
- **Severity**: High — checkpoint/resume is a documented feature that doesn't fully work.

## EI-12: Several D4 scenarios report Success but produce empty output (0ms) [OPEN]
- **Found in test**: [S14 debate 3ms], [S29 dead_path 2ms], [S04 email_drafter 1120ms unclear]
- **Problem**: Some scenarios return `ExecutionStatus::Success` with empty `output: {}` and near-zero elapsed time, meaning the workflow graph was parsed but no meaningful nodes executed. Likely caused by YAML wiring issues (edges not connecting properly) or graph validation silently passing invalid structures.
- **Expected behavior**: If no response node executes, the workflow should either fail validation or return an error, not silent empty Success.
- **Suggestion**: Add a post-execution check: if status is Success but output is empty and response node didn't execute, emit a warning or error.
- **Severity**: Medium — silent empty success is misleading.
