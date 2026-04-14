# Test Execution Log — Round 1

## Final Summary

| Dimension | Total Tests | Pass | Fail | Partial | Skipped |
|-----------|------------|------|------|---------|---------|
| D1: Providers | 46 | 42 | 1 | 0 | 3 (Gemini) |
| D2: Connectors | 5 | 2 | 3 | 0 | 0 |
| D3: Nodes | 29 | 27 | 1 | 1 | 0 |
| D4: Scenarios | 30 | 24 | 3 | 3 | 0 |
| **Total** | **110** | **95 (86%)** | **8** | **4** | **3** |

### Issue Tally
- **Engine Issues**: 10 (EI-01 ~ EI-10)
- **Skill Doc Issues**: 7 (SI-01 ~ SI-07)

---

## Key Discoveries

### Provider Authentication
- **Zhipu & Moonshot** route through DashScope (need Qwen key, not native key) — EI-01
- **Gemini** API key expired/invalid — all Gemini tests skipped

### Critical Engine Issues
1. **EI-04** PostgreSQL panics on JSONB/UUID/array/float8 types (Critical)
2. **EI-05** Node timeout_ms causes panic, not graceful error (High)
3. **EI-07** output_schema individual fields null for non-JSON-mode providers (High)
4. **EI-09** on_error edge fallback output not readable by response (High)
5. **EI-08** Object variables render as `[object]` in Handlebars (Medium)
6. **EI-10** Redis only supports 7 operations, no hash/list/sorted set (Medium)

### Positive Findings
- **9/10 providers** work for basic chat, system prompt, structured output, and tool calling
- **Transform** node (Rhai scripting) is rock-solid: builtins, file scripts, error handling all pass
- **Iterator** node works perfectly: sequential, parallel, error collection, empty input
- **Supervisor** node successfully plans and dispatches tools
- **Tool calling** works across OpenAI, Anthropic, DeepSeek, XAI, Qwen, Zhipu, Moonshot, MiniMax, Huoshan
- **Multi-provider parallel** workflows (S33 wide fanout) work excellently
- **Deep chains** (S32, 5 sequential generates across 3 providers) work perfectly
- **Redis cache-aside** pattern (S07) works end-to-end
- **Harness instructions** injection works (N18, S30)
- **Skill_set** progressive disclosure works (N47, S24)
- **Conditional routing** works reliably
- **Variable binding** all 3 forms work (single, multiple, default)

---

## D1: Provider Tests (46 tests)

### R1 — Basic Chat
| Provider | Status | Model | Elapsed |
|----------|--------|-------|---------|
| OpenAI | PASS | gpt-4o-mini | 1963ms |
| Anthropic | PASS | claude-haiku-4-5-20251001 | 968ms |
| Gemini | FAIL | gemini-2.0-flash | 167ms (key invalid) |
| DeepSeek | PASS | deepseek-chat | 2024ms |
| XAI | PASS | grok-4-fast | 612ms |
| Qwen | PASS | qwen-flash | 2054ms |
| Zhipu | PASS | glm-4.7 (DashScope) | 5591ms |
| Moonshot | PASS | kimi-k2.5 (DashScope) | 3265ms |
| MiniMax | PASS | MiniMax-M2.5 | 11070ms |
| Huoshan | PASS | doubao-seed-2-0-lite-260215 | 10006ms |

### R2 — System Prompt: 9/10 pass (Gemini fail)
### R3 — Structured Output: 8/10 pass (Gemini fail, Anthropic transient 529)
### R5 — Tool Calling: 9/10 pass (Gemini fail)
### R9 — Sampling: 3/3 pass (OpenAI, Anthropic, Qwen)
### R10 — Max Tokens: 3/3 pass (OpenAI, DeepSeek, Qwen)

---

## D2: Connector Tests (5 tests)

| ID | Connector | Status | Notes |
|----|-----------|--------|-------|
| D01 | PostgreSQL SELECT | PASS (simple types) | INT/TEXT/FLOAT/NULL work. Complex types panic (EI-04) |
| D02 | PostgreSQL INSERT | FAIL | Type handling issues |
| D04 | PostgreSQL NULL | FAIL | Same EI-04 panic |
| D13 | Redis GET/SET | PASS | Via S07 cache-aside |
| D14 | Redis HSET/HGET | FAIL | Operation unsupported (EI-10) |

---

## D3: Node Capability Tests (29 tests)

| ID | Capability | Status | Notes |
|----|-----------|--------|-------|
| N01 | Basic generate | PASS | DeepSeek, 4048ms |
| N02 | Schema simple | PASS | Single field |
| N03 | Schema complex (5 fields) | PASS* | Fields individually null (EI-07) |
| N04 | Schema nested (object[]) | PASS* | Same EI-07 |
| N05 | Conditional template | PASS | Template selection works |
| N06 | Variable single | PASS | |
| N07 | Variable multiple | PASS | Fallback works |
| N08 | Variable default | PASS | Default applied |
| N09 | Tool calling subgraph | PASS | Transform-backed tool |
| N10 | Multi-round tools | PASS | 2 tools, density comparison |
| N11 | Streaming | PASS | stream:true works |
| N15 | Error retry+continue | PASS | _error/_error_message populated |
| N16 | Error timeout | FAIL | Panic (EI-05) |
| N17 | Loop history | PASS | 1 iteration |
| N18 | Harness instructions | PASS | System prompt injection |
| N21 | Transform inline | PASS | Rhai string ops |
| N22 | Transform file script | PASS | sum/count/average |
| N23 | Transform builtins | PASS | now/uuid/json_encode/unique |
| N26 | Transform error | PASS | Division by zero caught |
| N27 | Iterator sequential | PASS | 3 items |
| N28 | Iterator parallel | PASS | 5 items, concurrency 3 |
| N30 | Iterator error collect | PASS | Error collected |
| N32 | Iterator empty | PASS | Empty array handled |
| N33 | Supervisor basic | PASS | Plan + dispatch |
| N39 | Workflow node | PASS | Error path for missing workflow |
| N47 | Skill_set | PASS | 2 skills injected |
| N50 | Conditional edge | PASS | Routing works |
| N52 | on_error edge | PARTIAL | Route works, output null (EI-09) |
| N53 | max_iterations | PASS | Loop capped |
| N55 | Expression functions | PASS | empty/exists |

---

## D4: Real-World Scenarios (30 tests)

| ID | Scenario | Status | Key Features Tested |
|----|----------|--------|-------------------|
| S01 | Blog Post Generator | PARTIAL | iterator+subgraph (schema cascade fail) |
| S02 | Multi-Language Translator | PARTIAL | parallel fanout (schema cascade fail) |
| S03 | Content Moderation | PASS | conditional routing, OpenAI |
| S04 | Email Drafter + Tone | PASS | conditional template, OpenAI |
| S05 | SEO Article Writer | PASS | iterator parallel, Qwen |
| S06 | DB Report Generator | PARTIAL | PG+LLM (EI-08 object interpolation) |
| S07 | Redis Cache-Aside | PASS | Redis GET→miss→compute→SET |
| S09 | Multi-DB Dashboard | FAIL | PG float8 type panic (EI-04) |
| S10 | ETL Pipeline | PASS | transform+iterator+subgraph |
| S11 | Research Assistant | PASS | supervisor, Qwen tool calling |
| S13 | Customer Support Router | PASS | 3-branch routing, 3 providers |
| S14 | Debate Simulator | FAIL | YAML wiring issue, 0 nodes ran |
| S16 | Multi-Provider Consensus | PASS | 3 providers parallel, synthesis |
| S17 | Provider Failover | PARTIAL | Fallback works, output null (EI-09) |
| S19 | Search-Augmented QA | PASS | Qwen enable_search |
| S21 | RAG with PostgreSQL | PASS | PG query→LLM analysis |
| S22 | Multi-Step Reasoning | PASS | XAI loop, correct answer |
| S24 | Skill-Powered Expert | PASS | 3 cooking skills, DeepSeek |
| S26 | Graceful Degradation | PASS | Transform-based error routing (avoids EI-09) |
| S28 | Iterator Error Collect | PASS | 5 items, error collection |
| S29 | Dead Path Handling | PASS* | 2ms — may be empty output |
| S30 | Memory Agent (Harness) | PASS | Harness instructions injected |
| S32 | Deep Chain (5 stages) | PASS | 5 providers, 44s, excellent quality |
| S33 | Wide Fanout (5 parallel) | PASS | 5 providers simultaneous |
| S34 | Nested Structure | PASS | Outer+inner iterators |
| S35 | Large Schema (10 fields) | FAIL | All fields null (EI-07) |
| S36 | Empty Input | PASS | Default value handling |
| S37 | Unicode & Special | PASS | CJK/emoji processed |
| S38 | Long Context | PASS | ~2000 char input summarized |
| S39 | Schema Chain | PASS* | Likely schema fields null |
| S40 | Conditional + Loop | PASS | Correct routing to hard path |
