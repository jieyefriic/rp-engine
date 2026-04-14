# rp-engine Ultimate Test Plan — Round 1

## Goal

Comprehensive, production-grade testing of the entire rp-engine: every provider, every node type, every data connector, and 30-50 real-world YAML workflow scenarios. Every test run is documented with purpose, YAML, execution result, and issues found.

## Environment

- Runtime: Claude remote virtual environment (Linux 4.4.0)
- Docker: Available (--iptables=false mode)
- Databases: Via Docker containers (PostgreSQL, MySQL, MongoDB, Redis, MinIO, Qdrant)
- API Keys: Via environment variables

## Directory Structure

```
ultimate-test/round-1/
  plan/
    TEST_PLAN.md           ← This file
  workflows/               ← All YAML workflow files
    dim1-providers/        ← Dimension 1: Provider tests
    dim2-connectors/       ← Dimension 2: Data connector tests
    dim3-nodes/            ← Dimension 3: Node capability tests
    dim4-scenarios/        ← Dimension 4: Real-world scenarios
  results/                 ← Execution results (JSON state dumps)
    dim1-providers/
    dim2-connectors/
    dim3-nodes/
    dim4-scenarios/
  issues/
    SKILL_ISSUES.md        ← Issues with skill.md documentation
    ENGINE_ISSUES.md       ← Issues with engine capabilities
    TEST_LOG.md            ← Running log of all test executions
  runner/                  ← Test runner Rust files
```

---

## Dimension 1: Provider Coverage (P-series)

Goal: Test every provider with every applicable request type.

### Provider Matrix

| ID | Provider | Models to Test | Env Var |
|----|----------|---------------|---------|
| P01 | OpenAI | gpt-4o-mini, gpt-4o | OPENAI_API_KEY |
| P02 | Anthropic | claude-sonnet-4-20250514 | ANTHROPIC_API_KEY |
| P03 | Gemini | gemini-2.0-flash | GEMINI_API_KEY |
| P04 | DeepSeek | deepseek-chat | DEEPSEEK_API_KEY |
| P05 | XAI | grok-3-mini | XAI_API_KEY |
| P06 | Qwen | qwen-plus, qwen-vl-plus | QWEN_API_KEY |
| P07 | Zhipu | glm-4-flash | ZHIPU_API_KEY |
| P08 | Moonshot | moonshot-v1-8k | MOONSHOT_API_KEY |
| P09 | MiniMax | MiniMax-Text-01 | MINIMAX_API_KEY |
| P10 | Huoshan | doubao-seed-1.6 (or available model) | HUOSHAN_API_KEY |
| P11 | OpenAI Compatible | (local/proxy endpoint or any compatible API) | OAI_COMPAT_API_KEY |

### Request Type Matrix

For each provider, test all applicable capabilities:

| ID | Request Type | Description | Applicable Providers |
|----|-------------|-------------|---------------------|
| R1 | Basic Chat | Simple user prompt → text response | ALL |
| R2 | System Prompt | system_prompt + user_prompt | ALL |
| R3 | Structured Output | output_schema → JSON response | ALL |
| R4 | Streaming | stream: true, verify token flow | ALL |
| R5 | Tool Calling | tools array + subgraph dispatch | OpenAI, Anthropic, Gemini, DeepSeek, Qwen, Zhipu |
| R6 | Vision/Multimodal | images input (base64, URL) | OpenAI, Anthropic, Gemini, Qwen |
| R7 | Web Search | enable_search: true | Qwen |
| R8 | Thinking/Reasoning | thinking_budget or reasoning_effort | Anthropic (claude), OpenAI (o-series) |
| R9 | Temperature/TopP | Various temperature + top_p combos | ALL |
| R10 | Max Tokens | Small max_tokens constraint | ALL |

### Test Files: `workflows/dim1-providers/`

Each test: `p{NN}_r{N}_{provider}_{type}.yaml`

Example: `p01_r1_openai_basic.yaml`, `p06_r5_qwen_tools.yaml`

---

## Dimension 2: Data Connector Coverage (D-series)

Goal: Test every data connector with real Docker services, covering various data types, operations, and edge cases.

### Docker Services

| Service | Image | Port | Container Name |
|---------|-------|------|----------------|
| PostgreSQL | postgres:16 | 5432 | ut-postgres |
| MySQL | mysql:8.0 | 3306 | ut-mysql |
| MongoDB | mongo:7 | 27017 | ut-mongodb |
| Redis | redis:7 | 6379 | ut-redis |
| MinIO (S3) | minio/minio | 9000 | ut-minio |
| Qdrant | qdrant/qdrant | 6333/6334 | ut-qdrant |

### Test Matrix

| ID | Connector | Operation | Data Types / Edge Cases |
|----|-----------|-----------|------------------------|
| D01 | PostgreSQL | SELECT | Basic query, multiple columns, various types (text, int, float, boolean, timestamp, jsonb, uuid, numeric) |
| D02 | PostgreSQL | INSERT + SELECT | Write then read back, verify round-trip |
| D03 | PostgreSQL | Parameterized query | Template variables in SQL queries |
| D04 | PostgreSQL | NULL handling | Nullable columns, NULL in results |
| D05 | PostgreSQL | Large result set | 100+ rows, verify no OOM |
| D06 | MySQL | SELECT | Basic query, MySQL-specific types (ENUM, SET, DATETIME) |
| D07 | MySQL | INSERT + SELECT | Write/read round-trip |
| D08 | MySQL | Various types | DECIMAL, BLOB, TEXT, JSON column type |
| D09 | MongoDB | find | Basic query, nested documents |
| D10 | MongoDB | insertOne + find | Write/read round-trip |
| D11 | MongoDB | Aggregation | Pipeline with $group, $match |
| D12 | MongoDB | Nested/array data | Documents with arrays and nested objects |
| D13 | Redis | GET/SET | String values |
| D14 | Redis | HSET/HGET | Hash operations |
| D15 | Redis | JSON operations | RedisJSON if available |
| D16 | Redis | LIST operations | LPUSH/LRANGE |
| D17 | S3 (MinIO) | PUT + GET | Upload and download objects |
| D18 | S3 (MinIO) | LIST | List objects in bucket |
| D19 | Qdrant | Upsert + Search | Vector insert + similarity search |
| D20 | REST API | GET | External HTTP API call |
| D21 | REST API | POST with body | JSON body POST |
| D22 | GraphQL | Query | GraphQL query execution |

### Test Files: `workflows/dim2-connectors/`

Each test: `d{NN}_{connector}_{operation}.yaml`

---

## Dimension 3: Node Capability Coverage (N-series)

Goal: Every node type's every capability, tested individually.

### 3.1 generate (N01-N20)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N01 | Basic generate | Minimal: provider + model + template → output |
| N02 | output_schema simple | Single string field |
| N03 | output_schema complex | Multiple fields: string, number, boolean, array |
| N04 | output_schema nested | object[] with children fields |
| N05 | Conditional template | TemplateSelector with conditions + default |
| N06 | Variable binding: single | `var: "node.field"` |
| N07 | Variable binding: multiple | `var: ["path1", "path2"]` (first available) |
| N08 | Variable binding: default | `var: {from: "path", default: "fallback"}` |
| N09 | Tool calling (subgraph) | tools + ref + subgraph execution |
| N10 | Tool calling multi-round | max_tool_rounds, LLM calls tool multiple times |
| N11 | Streaming | stream: true, verify _ttft_ms in output |
| N12 | Vision/images | images variable binding, base64 input |
| N13 | Skill injection | skill_sets reference, load_skill tool |
| N14 | MCP tool injection | mcp_tool_sets reference |
| N15 | Error handling: retry | error_handling: {retry: 2, on_exhausted: continue} |
| N16 | Error handling: timeout | error_handling: {timeout_ms: 100} (force timeout) |
| N17 | Conversation history | Array accumulation `node[*].field` in loops |
| N18 | Harness injection | harness.instructions in system_prompt |
| N19 | No template fallback | Variables used as prompt without template |
| N20 | capture_actual_prompt | Verify _actual_system_prompt and _actual_user_prompt |

### 3.2 transform (N21-N26)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N21 | Inline script | `script:` with Rhai code returning a map |
| N22 | File script | `file:` referencing transforms section |
| N23 | Built-in functions | now(), uuid(), json_encode(), unique() |
| N24 | Type coercions | JSON → Rhai → JSON round-trip for all types |
| N25 | Array/map processing | Complex data transformation |
| N26 | Error handling | Division by zero, exceeding limits |

### 3.3 iterator (N27-N32)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N27 | Sequential iteration | Items array, subgraph per item |
| N28 | Parallel iteration | parallel: true, max_concurrency |
| N29 | Index variable | index_var usage in subgraph |
| N30 | on_error: collect | Error in one item doesn't stop others |
| N31 | on_error: fail_fast | Error in one item stops all |
| N32 | Empty/null items | Edge cases: [], null, non-array |

### 3.4 supervisor (N33-N38)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N33 | Basic planning | Planner → todo list → tool dispatch |
| N34 | Parallel execution | parallel: true, verify concurrent dispatch |
| N35 | Exit conditions | max_iterations, timeout, tool_failures |
| N36 | Dependency chains | action_prerequest ordering |
| N37 | Tool retry | max_retry_per_tool behavior |
| N38 | Exit edge routing | exit_reason edges (todo_empty, max_iterations, default) |

### 3.5 workflow (N39-N41)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N39 | Basic sub-workflow | Nested workflow execution |
| N40 | Variable passing | Parent → child variable mapping |
| N41 | capture_child_state | Full child execution record capture |

### 3.6 checkpoint (N42-N43)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N42 | Pause + Resume | Checkpoint pauses, resume continues |
| N43 | Resume with new input | Merge new input on resume |

### 3.7 MCP nodes (N44-N46)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N44 | mcp explicit call | Direct MCP tool invocation |
| N45 | mcp_tools discovery | Tool list from MCP server |
| N46 | MCP + generate | LLM-driven MCP tool selection |

### 3.8 skill_set (N47-N49)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N47 | Basic skill injection | Skills in system_prompt |
| N48 | Content file | content_file path reading |
| N49 | References | load_skill_reference tool |

### 3.9 Edges & Routing (N50-N55)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N50 | Conditional edge | `condition: node.field == "value"` |
| N51 | Default edge | is_default: true fallback |
| N52 | on_error edge | Route to error handler on failure |
| N53 | max_iterations | Loop protection |
| N54 | Dead path detection | Unreachable nodes skipped |
| N55 | Expression functions | empty(), exists(), len(), contains operators |

### 3.10 State & Variables (N56-N58)

| ID | Capability | What to Test |
|----|-----------|--------------|
| N56 | Array accumulation | node[*].field in loops |
| N57 | Nested path access | node.field.subfield |
| N58 | Short ID references | node.arr[abc] |

### Test Files: `workflows/dim3-nodes/`

Each test: `n{NN}_{node}_{capability}.yaml`

---

## Dimension 4: Real-World Scenarios (S-series)

Goal: 30-50 production-grade workflows that exercise multiple features together in realistic use cases.

### Content & Writing

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S01 | Blog Post Generator | start → generate(outline) → iterator(sections) → generate(compile) → response | iterator + subgraph + multi-step |
| S02 | Multi-Language Translator | start → generate(detect_lang) → parallel generates(3 languages) → transform(merge) → response | parallel, transform, output_schema |
| S03 | Content Moderation Pipeline | start → generate(classify) → conditional(safe/unsafe) → generate(response/rejection) → response | conditional routing, output_schema |
| S04 | Email Drafter with Tone Control | start → generate(analyze_tone) → conditional template(formal/casual) → generate(draft) → response | conditional template selector |
| S05 | SEO Article Writer with Keyword Research | start → generate(keywords) → generate(outline) → iterator(write_sections) → generate(compile_with_seo) → response | iterator, multi-step chaining |

### Data Analysis & Processing

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S06 | Database Report Generator | start → data_connector(pg_query) → generate(analyze) → generate(report) → response | PostgreSQL, LLM analysis |
| S07 | Redis Cache-Aside Pattern | start → data_connector(redis_get) → conditional(hit/miss) → generate(compute) → data_connector(redis_set) → response | Redis, conditional, write-back |
| S08 | MongoDB Log Analyzer | start → data_connector(mongo_aggregate) → transform(process) → generate(summarize) → response | MongoDB, transform, analysis |
| S09 | Multi-DB Dashboard | start → parallel[pg_query, redis_stats, mongo_count] → transform(merge) → generate(narrative) → response | parallel DB access, transform merge |
| S10 | CSV-to-DB ETL Pipeline | start → transform(parse_csv) → iterator(insert_rows, pg) → transform(stats) → response | transform, iterator, PostgreSQL |

### Multi-Agent & Orchestration

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S11 | Research Assistant | supervisor(planner) → [web_search_tool, summarize_tool] → generate(final_answer) | supervisor, tool dispatch |
| S12 | Code Review Pipeline | start → generate(analyze_code) → supervisor(review_planner) → [style_check, security_check, perf_check] → generate(report) | supervisor parallel, multi-tool |
| S13 | Customer Support Router | start → generate(classify_intent) → conditional(billing/tech/general) → generate(specialized_response) → response | multi-branch routing |
| S14 | Debate Simulator | start → loop[generate(pro_argument) → generate(con_argument) → generate(judge)] → response | loop, multi-perspective |
| S15 | Document QA with Context | start → data_connector(pg_search) → generate(answer) → generate(verify) → conditional(confident/unsure) → response | DB retrieval, verification loop |

### Provider-Specific Features

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S16 | Multi-Provider Consensus | start → parallel[openai_gen, anthropic_gen, qwen_gen] → generate(synthesize) → response | 3 providers, parallel, comparison |
| S17 | Provider Failover Chain | start → generate(primary) → on_error → generate(fallback) → response | error handling, on_error edge |
| S18 | Thinking + Analysis | start → generate(deep_think, thinking_budget) → transform(extract) → response | Anthropic thinking mode |
| S19 | Search-Augmented QA | start → generate(qwen, enable_search) → generate(verify_with_search) → response | Qwen web search |
| S20 | Vision Analysis Pipeline | start → generate(describe_image) → generate(analyze_description) → response | multimodal vision |

### Complex Workflows

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S21 | RAG Pipeline | start → data_connector(qdrant_search) → generate(answer_with_context) → response | vector DB, RAG pattern |
| S22 | Multi-Step Reasoning | start → loop[generate(think) → transform(check_confidence)] → generate(final) → response | loop + transform gate |
| S23 | Workflow-in-Workflow | start → workflow(sub_workflow_1) → generate(process) → workflow(sub_workflow_2) → response | nested workflows |
| S24 | Skill-Powered Expert System | start → skill_set(domain_skills) → generate(expert_answer, skill_sets) → response | skill progressive disclosure |
| S25 | MCP Tool Agent | start → mcp_tools(discover) → generate(with_mcp_tools) → response | MCP integration end-to-end |

### Error Handling & Resilience

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S26 | Graceful Degradation | start → generate(primary, timeout_ms:500) → on_error → generate(simple_fallback) → response | timeout + error edge |
| S27 | Retry with Backoff | start → generate(flaky_call, retry:3) → response | retry mechanism |
| S28 | Iterator Error Collection | start → iterator(items, on_error:collect) → transform(filter_errors) → response | partial failure handling |
| S29 | Dead Path Handling | start → generate(gate) → conditional(path_a/path_b) → response (captures both) | dead path detection |

### Harness & Memory

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S30 | Persistent Memory Agent | start → generate(with_memory) → response + harness writeback | memory read + writeback |
| S31 | Multi-File Harness | start → generate(with_harness_files) → response | harness.files + harness.instructions |

### Edge Cases & Stress

| ID | Scenario | Nodes Used | Key Features |
|----|----------|-----------|--------------|
| S32 | Deep Chain (10+ nodes) | start → gen1 → gen2 → ... → gen10 → response | deep sequential chain |
| S33 | Wide Fanout (5+ parallel) | start → parallel[gen1...gen5] → transform(merge) → response | wide parallelism |
| S34 | Nested Loops | start → loop[gen → loop[inner_gen]] → response | nested iteration |
| S35 | Large Output Schema | start → generate(complex_schema, 15+ fields) → response | complex structured output |
| S36 | Empty Input Handling | start(no input) → generate(handle_empty) → response | graceful empty input |
| S37 | Unicode & Special Characters | start → generate(unicode_prompt) → response | CJK, emoji, special chars |
| S38 | Long Prompt | start → generate(very_long_context) → response | context window stress |
| S39 | Multiple Output Schemas in Chain | start → gen1(schema_a) → gen2(schema_b, uses gen1 fields) → gen3(schema_c) → response | schema chaining |
| S40 | Conditional + Loop Combo | start → gen(classify) → conditional → loop[gen(iterate)] → response | complex routing + loops |

---

## Test Execution Protocol

### For each test:

1. **Write YAML** in `workflows/dim{N}-xxx/`
2. **Write runner** (Rust example) that executes the YAML and saves state JSON to `results/`
3. **Execute** via `cargo run --example`
4. **Analyze** result using a sub-agent (to avoid context overflow from large state JSONs)
5. **Record** in `issues/TEST_LOG.md`:
   - Test ID, purpose, timestamp
   - Status: PASS / FAIL / PARTIAL
   - If FAIL: root cause classification
     - `[YAML]` — YAML authoring issue → record in `SKILL_ISSUES.md`
     - `[ENGINE]` — Engine bug or limitation → record in `ENGINE_ISSUES.md`
     - `[PROVIDER]` — Provider API issue (rate limit, model unavailable, etc.)
     - `[ENV]` — Environment issue (Docker, network, etc.)
   - Key observations

### State Analysis Protocol

- **Never** read full state JSON in main context
- Use sonnet sub-agent with specific analysis instructions
- Sub-agent reports: status, output fields, token usage, timing, errors

---

## Execution Order

1. **Infra Setup**: Docker containers for all databases
2. **Dimension 1**: Provider tests (P01-P11 × R1-R10) — establishes provider availability
3. **Dimension 3**: Node capability tests (N01-N58) — validates individual features
4. **Dimension 2**: Data connector tests (D01-D22) — requires Docker services
5. **Dimension 4**: Real-world scenarios (S01-S40) — combines everything

This order ensures we discover broken providers/features early before combining them in complex scenarios.

---

## Success Criteria

- All provider basic chat tests pass (P×R1)
- All node types have at least one passing test
- All data connectors successfully connect and execute operations
- 80%+ of real-world scenarios execute successfully
- All failures are classified and documented with actionable suggestions
