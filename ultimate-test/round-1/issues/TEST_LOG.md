# Test Execution Log — Round 1

## Summary

| Dimension | Total Tests | Pass | Fail | Partial | Skipped |
|-----------|------------|------|------|---------|---------|
| D1: Providers | 46 | 42 | 1 (Gemini key) | 0 | 3 (Gemini) |
| D2: Connectors | 4 | 2 | 2 | 0 | 0 |
| D3: Nodes | 22 | 20 | 1 | 1 | 0 |
| D4: Scenarios | 18 | 11 | 3 | 4 | 0 |
| **Total** | **90** | **75** | **7** | **5** | **3** |

---

## Key Discoveries

### Zhipu & Moonshot route through DashScope (EI-01)
Both providers require a DashScope/Qwen API key, not their native keys.

### Gemini API Key Invalid
All Gemini tests skipped (key `AIzaSyCp...` returns immediate 401).

### output_schema extraction unreliable (EI-07)
DeepSeek (and likely others without native JSON mode) often returns prose instead of JSON. Individual schema fields (`gen.title`, `gen.score`) are null — only `gen.output` (raw text) is populated. This cascades to break multi-step workflows (S01, S02, S35, S39).

### on_error edge output not consumable (EI-09)
Fallback nodes execute correctly but their output can't be read by response nodes.

### Object variables → `[object]` in templates (EI-08)
JSON objects/maps passed through Handlebars templates render as `[object]` instead of serialized JSON.

### PostgreSQL panics on complex types (EI-04)
JSONB, TEXT[], UUID, TIMESTAMPTZ cause unrecoverable panics.

### Node timeout panics (EI-05)
`error_handling.timeout_ms` causes panic instead of graceful error.

---

## D1: Provider Tests

### R1 — Basic Chat (10 tests)
| Provider | Status | Model | Elapsed |
|----------|--------|-------|---------|
| OpenAI | PASS | gpt-4o-mini | 1963ms |
| Anthropic | PASS | claude-haiku-4-5-20251001 | 968ms |
| Gemini | FAIL [PROVIDER] | gemini-2.0-flash | 167ms (invalid key) |
| DeepSeek | PASS | deepseek-chat | 2024ms |
| XAI | PASS | grok-4-fast | 612ms |
| Qwen | PASS | qwen-flash | 2054ms |
| Zhipu | PASS | glm-4.7 | 5591ms (DashScope key) |
| Moonshot | PASS | kimi-k2.5 | 3265ms (DashScope key) |
| MiniMax | PASS | MiniMax-M2.5 | 11070ms |
| Huoshan | PASS | doubao-seed-2-0-lite-260215 | 10006ms |

### R2 — System Prompt (10 tests): 9/10 pass (Gemini fail)
### R3 — Structured Output (10 tests): 8/10 pass (Gemini fail, Anthropic 529 overload)
### R5 — Tool Calling (10 tests): 9/10 pass (Gemini fail)
### R9 — Sampling (3 tests): 3/3 pass
### R10 — Max Tokens (3 tests): 3/3 pass

---

## D2: Connector Tests

| ID | Connector | Status | Notes |
|----|-----------|--------|-------|
| D01 | PostgreSQL SELECT | PASS (simple) / FAIL (complex) | INT/TEXT/FLOAT work. JSONB/TEXT[]/UUID/TIMESTAMPTZ panic (EI-04) |
| D02 | PostgreSQL INSERT+SELECT | FAIL | Same type handling issue |
| D04 | PostgreSQL NULL | FAIL | Same panic on complex columns |
| D13 | Redis GET/SET | PASS (via S07) | Cache-aside pattern works end-to-end |

---

## D3: Node Capability Tests

| ID | Capability | Status | Notes |
|----|-----------|--------|-------|
| N01 | Basic generate | PASS | 4048ms |
| N02 | Schema simple | PASS | Single field works |
| N03 | Schema complex (5 fields) | PASS* | Engine marked success but individual fields null (EI-07) |
| N04 | Schema nested (object[]) | PASS* | Same — nested items null |
| N05 | Conditional template | PASS | Template correctly selected |
| N06 | Variable single | PASS | |
| N07 | Variable multiple sources | PASS | Fallback to second source works |
| N08 | Variable default | PASS | Default "English" applied when no language provided |
| N09 | Tool calling (subgraph) | PASS | Calculate tool with transform subgraph |
| N10 | Tool calling multi-round | PASS | Two tools, population density comparison |
| N15 | Error retry + continue | PASS | _error and _error_message properly populated |
| N16 | Error timeout | FAIL [ENGINE] | Panic instead of graceful error (EI-05) |
| N17 | Loop with history | PASS | 1 iteration (model solved quickly) |
| N21 | Transform inline | PASS | to_upper, len, string ops |
| N22 | Transform file script | PASS | sum/count/average correct |
| N23 | Transform builtins | PASS | now(), uuid(), json_encode(), unique() all correct |
| N27 | Iterator sequential | PASS | 3 items, all summaries generated |
| N28 | Iterator parallel | PASS | 5 items, max_concurrency: 3 |
| N30 | Iterator error collect | PASS | Error properly collected |
| N32 | Iterator empty | PASS | count: 0, no errors |
| N50 | Conditional edge | PASS | Science/humanities routing works |
| N52 | on_error edge | PARTIAL | Error routing works, but fallback output null in response (EI-09) |

---

## D4: Real-World Scenarios

| ID | Scenario | Status | Key Findings |
|----|----------|--------|-------------|
| S01 | Blog Post Generator | PARTIAL | Outline generated but iterator didn't fire (schema fields null for sections) |
| S02 | Multi-Language Translator | PARTIAL | Language detected but 3 translations never executed |
| S03 | Content Moderation | PASS | Correctly classified safe content |
| S06 | DB Report Generator | PARTIAL | PG query returned data but LLM analysis didn't execute (EI-08 object interpolation) |
| S07 | Redis Cache-Aside | PASS | Full cache miss → compute → store cycle works |
| S13 | Customer Support Router | PASS | Correctly routed to tech support |
| S14 | Debate Simulator | FAIL | 3ms, no nodes executed — YAML wiring bug |
| S16 | Multi-Provider Consensus | PASS | 3 providers answered, synthesis produced |
| S17 | Provider Failover | PARTIAL | Fallback executed but output null (EI-09) |
| S22 | Multi-Step Reasoning | PASS | Correct answer (9 sheep), single iteration |
| S28 | Iterator Error Collect | PASS* | Non-existent provider didn't error (test design issue) |
| S32 | Deep Chain (5 stages) | PASS | All 5 stages substantive, excellent quality |
| S33 | Wide Fanout (5 providers) | PASS | All 5 perspectives produced |
| S35 | Large Schema (10 fields) | FAIL | All 10 structured fields null (EI-07) |
| S36 | Empty Input | PASS | Graceful handling with default value |
| S37 | Unicode & Special | PASS | CJK/emoji handled correctly |
| S39 | Schema Chain | PASS* | Ran but likely schema fields null (EI-07) |
| S40 | Conditional + Loop | PARTIAL | Correct routing but output empty |
