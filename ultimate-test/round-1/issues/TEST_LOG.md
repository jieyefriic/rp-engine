# Test Execution Log — Round 1

## Final Summary (Post-Fix)

| Dimension | Total Tests | Pass | Fail | Partial | Skipped |
|-----------|------------|------|------|---------|---------|
| D1: Providers | 46 | 46 | 0 | 0 | 0 |
| D2: Connectors | 6 | 4 | 2 | 0 | 0 |
| D3: Nodes | 32 | 30 | 0 | 2 | 0 |
| D4: Scenarios | 31 | 25 | 2 | 4 | 0 |
| **Total** | **115** | **105 (91%)** | **4** | **6** | **0** |

### Compared to Pre-Fix
- Pre-fix: 95/110 (86%) pass
- Post-fix: 105/115 (91%) pass — 10+ new tests added, pass rate up 5%

### Issue Tally
- **Engine Issues**: 12 total (EI-01~EI-12) — 11 fixed, 1 info-only
- **Skill Doc Issues**: 8 total (SI-01~SI-08) — 8/8 fixed

### Fix Summary
- **Phase 1** (commit 065f951): Fixed EI-01~EI-10 (code + docs)
- **Phase 2** (commit 989e518): Fixed EI-07 root cause (YAML placement), EI-09 (inputs_satisfied), EI-11/12
- **Skill docs** (commit 4809dec): All SI-01~SI-08 resolved in skill.md

---

## D1: Provider Tests (46 tests) — ALL PASS

### R1 — Basic Chat (10/10 pass)
| Provider | Model | Elapsed | Notes |
|----------|-------|---------|-------|
| OpenAI | gpt-4o-mini | 1963ms | |
| Anthropic | claude-haiku-4-5-20251001 | 968ms | |
| Gemini | gemini-2.0-flash | 594ms | Key works (was transient 401 earlier) |
| DeepSeek | deepseek-chat | 2024ms | |
| XAI | grok-4-fast | 612ms | |
| Qwen | qwen-flash | 2054ms | |
| Zhipu | glm-4.7 | 5591ms | Via DashScope (needs Qwen key) |
| Moonshot | kimi-k2.5 | 3265ms | Via DashScope (needs Qwen key) |
| MiniMax | MiniMax-M2.5 | 11070ms | |
| Huoshan | doubao-seed-2-0-lite-260215 | 10006ms | |

### R2 — System Prompt: 10/10 pass
### R3 — Structured Output: 10/10 pass
### R5 — Tool Calling: 10/10 pass
### R9 — Sampling: 3/3 pass (OpenAI, Anthropic, Qwen)
### R10 — Max Tokens: 3/3 pass (OpenAI, DeepSeek, Qwen)

---

## D2: Connector Tests (6 tests)

| ID | Connector | Status | Notes |
|----|-----------|--------|-------|
| D01 | PostgreSQL SELECT | PASS | All types work post-fix (EI-04) incl. JSONB, arrays, UUID |
| D02 | PostgreSQL INSERT | PASS | Round-trip write/read |
| D04 | PostgreSQL NULL | PASS | NULL handling correct |
| D13 | Redis GET/SET | PASS | Cache-aside pattern works |
| D14 | Redis HSET/HGETALL | PASS | Post-fix (EI-10) |
| D16 | Redis LPUSH/LRANGE | FAIL | Test YAML needs update for new API |

---

## D3: Node Capability Tests (32 tests)

| ID | Capability | Status | Notes |
|----|-----------|--------|-------|
| N01 | Basic generate | PASS | |
| N02 | Schema simple | PASS | |
| N03 | Schema complex (5 fields) | PASS | Post-fix: all 5 fields correct (title, score:9.2, tags, is_valid, details) |
| N04 | Schema nested (object[]) | PASS | Post-fix: items array populated |
| N05 | Conditional template | PASS | |
| N06 | Variable single | PASS | |
| N07 | Variable multiple sources | PASS | |
| N08 | Variable default | PASS | |
| N09 | Tool calling (subgraph) | PASS | |
| N10 | Tool calling multi-round | PASS | |
| N11 | Streaming | PASS | 9 chunks, _ttft_ms present |
| N11+ | Streaming (advanced) | PASS | Via ut_advanced runner |
| N15 | Error retry + continue | PASS | |
| N16 | Error timeout | PASS | Post-fix (EI-05): 100ms timeout works |
| N17 | Loop with history | PASS | |
| N18 | Harness instructions | PASS | |
| N21 | Transform inline | PASS | |
| N22 | Transform file script | PASS | |
| N23 | Transform builtins | PASS | |
| N26 | Transform error | PASS | Division by zero caught |
| N27 | Iterator sequential | PASS | |
| N28 | Iterator parallel | PASS | |
| N30 | Iterator error collect | PASS | |
| N32 | Iterator empty | PASS | |
| N33 | Supervisor basic | PASS | |
| N39 | Workflow node (error) | PASS | Missing workflow error caught |
| N39+ | Sub-workflow (advanced) | PASS | Parent→child→enhance full chain |
| N42 | Checkpoint/Resume | PASS | Post-fix: Phase 1 pause + Phase 2 resume with evaluation |
| N47 | Skill_set | PASS | |
| N50 | Conditional edge | PASS | |
| N52 | on_error edge | PASS | Post-fix (EI-09): fallback output now accessible |
| N53 | max_iterations | PASS | |
| N55 | Expression functions | PASS | |

---

## D4: Real-World Scenarios (31 tests)

| ID | Scenario | Status | Notes |
|----|----------|--------|-------|
| S01 | Blog Post Generator | PARTIAL | Outline works, iterator may still have schema cascade issue |
| S02 | Multi-Language Translator | PARTIAL | Language detection works, translations need retest |
| S03 | Content Moderation | PASS | |
| S04 | Email Drafter + Tone | PASS | |
| S05 | SEO Article Writer | PASS | |
| S06 | DB Report Generator | PASS | PG query → LLM analysis works |
| S07 | Redis Cache-Aside | PASS | Full miss→compute→store cycle |
| S09 | Multi-DB Dashboard | FAIL | PG aggregate query type issue |
| S10 | ETL Pipeline | PASS | transform+iterator+subgraph |
| S11 | Research Assistant | PASS | supervisor + tool dispatch |
| S13 | Customer Support Router | PASS | 3-branch routing, 3 providers |
| S14 | Debate Simulator | PARTIAL | YAML wiring issue — needs investigation |
| S16 | Multi-Provider Consensus | PASS | 3 providers + synthesis, JSON rendering fixed |
| S17 | Provider Failover | PASS | Post-fix: fallback output correctly surfaced |
| S19 | Search-Augmented QA | PASS | Qwen enable_search |
| S21 | RAG with PostgreSQL | PASS | PG query → LLM answer |
| S22 | Multi-Step Reasoning | PASS | Correct answer (9 sheep) |
| S24 | Skill-Powered Expert | PASS | 3 cooking skills |
| S26 | Graceful Degradation | PASS | Transform-based error routing |
| S28 | Iterator Error Collect | PASS | |
| S29 | Dead Path Handling | PARTIAL | May need YAML wiring fix |
| S30 | Memory Agent (Harness) | PASS | |
| S32 | Deep Chain (5 stages) | PASS | Excellent quality, 5 providers |
| S33 | Wide Fanout (5 parallel) | PASS | All 5 perspectives produced |
| S34 | Nested Structure | PASS | Outer+inner iterators |
| S35 | Large Schema (10 fields) | PASS | Post-fix: all 10 fields populated |
| S36 | Empty Input | PASS | |
| S37 | Unicode & Special | PASS | |
| S38 | Long Context | PASS | ~2000 char input summarized |
| S39 | Schema Chain | PASS | Post-fix: schema fields flow between nodes |
| S40 | Conditional + Loop | FAIL | YAML wiring issue |
