# rp-engine Architecture Snapshot — 2026-04-10

A grounding reference for what actually exists in the codebase today. Every
claim points to `file:line` so you can verify quickly. Doc folder may be stale;
this snapshot was reconstructed by reading source.

Relative paths below are rooted at
`/Users/jieye/Desktop/focus/rp-workspace/rp-engine`.

---

## 1. Node Type Inventory

### 1.1 Dispatch table — where a node type maps to an executor

Two-level dispatch lives in `src/engine/executor.rs:993-1031` (`execute_node_once`):

- `NodeType::Custom(type_name)` → looks up `config.node_registry` and runs a
  `CustomNode` trait object.
- Everything else → `crate::nodes::create_executor(&node.node_type)` in
  `src/nodes/mod.rs:401-416`.

Built-in match arms in `create_executor`:

```
Start         → StartExecutor
Response      → ResponseExecutor
Checkpoint    → CheckpointExecutor
Generate      → GenerateExecutor
DataConnector → DataConnectorExecutor
Supervisor    → SupervisorExecutor
Transform     → TransformExecutor
Search        → SearchExecutor
Iterator      → iterator::IteratorExecutor
Execute       → NotImplementedExecutor("execute")   // stub
Mcp           → NotImplementedExecutor("mcp")       // stub
Custom(name)  → CustomNodeNotFoundExecutor(name)    // fallback if not registered
```

The `NodeType` enum itself is defined at `src/schema/node.rs:301-329` with
`#[serde(rename_all = "snake_case")]`. Note there is **no `Workflow` variant**:
the string `type: workflow` in YAML falls through to `NodeType::Custom("workflow")`
and is dispatched via the `NodeRegistry`. `EngineBuilder::build` auto-registers
`WorkflowNode` under that name at `src/engine/builder.rs:287-288`.

### 1.2 Node type table

| Type            | Executor struct           | File                                   | Status |
|-----------------|---------------------------|----------------------------------------|--------|
| `start`         | `StartExecutor`           | `src/nodes/start.rs:14`                | full |
| `response`      | `ResponseExecutor`        | `src/nodes/response.rs:14`             | full |
| `checkpoint`    | `CheckpointExecutor`      | `src/nodes/checkpoint.rs:19`           | full |
| `generate`      | `GenerateExecutor`        | `src/nodes/generate.rs:18`             | full (see §2) |
| `transform`     | `TransformExecutor`       | `src/nodes/transform.rs:16`            | full |
| `search`        | `SearchExecutor`          | `src/nodes/search.rs:39`               | full |
| `iterator`      | `IteratorExecutor`        | `src/nodes/iterator.rs:20`             | full |
| `supervisor`    | `SupervisorExecutor`      | `src/nodes/supervisor.rs:186`          | full |
| `data_connector`| `DataConnectorExecutor`   | `src/nodes/data_connector.rs:17`       | full (8 kinds) |
| `workflow`      | `WorkflowNode` (custom)   | `src/nodes/workflow_node.rs:38`        | full |
| `execute`       | `NotImplementedExecutor`  | `src/nodes/mod.rs:436`                 | not_implemented |
| `mcp`           | `NotImplementedExecutor`  | `src/nodes/mod.rs:436`                 | not_implemented |
| `<custom>`      | user-supplied `CustomNode`| trait in `src/nodes/mod.rs:330`        | full (pluggable) |

### 1.3 Per-node behaviour

**start** — `src/nodes/start.rs:17-41`.
Purpose: entry point; projects workflow input into named output fields.
Reads: `config.output: HashMap<String,String>` (field → state path).
Writes: each configured field; if empty, pass through all inputs verbatim.
Notable: no provider calls; trivial.

**response** — `src/nodes/response.rs:17-44`.
Purpose: final output shaping. Reads `config.output` map. For each entry,
reads a state path and writes that field to its outputs. Missing paths become
`Value::Null` (logged at debug). These outputs become the workflow's
`ExecutionResult.output` (captured in the executor loop at
`src/engine/executor.rs:733-735`).

**checkpoint** — `src/nodes/checkpoint.rs:22-48`.
Purpose: identical shaping logic to ResponseExecutor, but the engine treats
this node type specially: after executing, it produces a `CheckpointState`
and returns `ExecutionStatus::Paused` (`src/engine/executor.rs:711-730`).
Resume via `Engine::resume()`.

**transform** — `src/nodes/transform.rs:124-227`.
Purpose: pure-computation Rhai script.
Reads: `config.script` (inline) or `config.file` (name in `config.transforms`);
also resolves `config.variables` from state and injects them as scope vars plus
an aggregated `_vars` map.
Writes: if script returns a map → each key becomes an output field; otherwise
wraps the value as `{ "result": value }`.
Notable: `rhai::Engine` with sandboxing limits (100k ops, 1 MB strings, 10k
arrays/maps, `src/nodes/transform.rs:83-87`); registers helpers `now()`,
`uuid()`, `json_encode()`, `unique()`.

**search** — `src/nodes/search.rs:42-` (rest of file).
Purpose: calls a registered `SearchProvider` (Tavily / Brave / Bocha /
Perplexity — see `src/providers/search/`).
Reads: `config.provider` (provider id), `config.variables.query`,
`config.operation` for `max_results`, `freshness`, `include_answer`, etc.
Writes: `results` (array), `answer` (string or null), `query`.

**iterator** — `src/nodes/iterator.rs:36-146`.
Purpose: map an array through a subgraph, sequential or concurrent.
Reads: `config.items` (VariableBinding → must resolve to array),
`config.subgraph` (required), `config.item_var` (default `item`),
`config.index_var` (optional), `config.variables` (shared vars),
`config.parallel`, `config.max_concurrency` (default 5),
`config.max_iterations` (safety cap, default 1000), `config.on_error`
(`fail_fast` | `collect`).
Writes: `results` (array preserving input order), `count`, `success_count`,
`error_count`, `errors` (when present), `_usage` with aggregated tokens
(`build_outputs` at `src/nodes/iterator.rs:288`).
Notable: each iteration runs through `subgraph_runner::execute`; parallel
mode uses a semaphore-style bounded execution.

**supervisor** — `src/nodes/supervisor.rs:189-415`.
Purpose: planner-executor loop. LLM generates a todo list, tools (subgraphs)
execute each todo, results feed back into context.
Reads: `provider`, `model`, `template`, `tools: Vec<ToolRef>` (each
ToolRef → subgraph name + optional JSON schema), `max_iterations` (default
10), `max_retry_per_tool` (default 3), `timeout` (ms, default 300000),
`max_result_length` (default 1M), `fail_on_tool_count`, `parallel`,
`summarizer_template`.
Writes: `task_result` (summary + planner usage + actions + stats),
`exit_reason` (one of `TodoEmpty`, `MaxIterations`, `Timeout`,
`MaxResultLength`, `ToolFailures`, `Default`), `_usage`, and one
`action_<short_id>` entry per executed tool call.
Notable: exit_reason drives edge routing — `evaluate_edges` in
`src/engine/executor.rs:1044-1099` first tries exact `exit_reason` matches,
then `ExitReason::Default` exit edges, before falling through to generic
edge evaluation.

**data_connector** — `src/nodes/data_connector.rs:27-`.
Purpose: call external datasources.
Reads: `config.data_source_id` (references a `DataSourceDef` from
`workflow.data_sources` or engine-level), `config.operation` (kind-specific
parameters), `config.variables`.
Writes: kind-specific. REST/GraphQL write `status`, `data`, `headers`;
SQL writes `data` (rows), etc.
Dispatch: `data_source.kind` string →
`rest_api | graphql | s3 | minio | postgresql | postgres | qdrant | redis |
mysql | mariadb | mongodb | mongo` (match at
`src/nodes/data_connector.rs:73-91`). Connectors live in
`src/nodes/connectors/`. Pool reuse: PostgreSQL pools and Redis connection
managers are stored in `ConnectionPools`
(`src/engine/executor.rs:70-166`).

**workflow** (custom-registered) — `src/nodes/workflow_node.rs:53-141`.
Purpose: run another prepared workflow as a sub-workflow with isolated
state.
Reads: `config.workflow` (workflow name), `config.variables` (parent state
paths → sub-workflow input), `config.output` (map sub outputs back to
parent state), and special `capture_child_state` variable to opt into
child execution history capture.
Writes: `_status` (debug form of `ExecutionStatus`), `_timing_ms`, `_error`
on failure, `_child_executions` when capture is enabled, plus whatever the
user-declared output mapping copies over.
Notable: constructs a new `ExecutionConfig` cloned from parent
(`src/nodes/workflow_node.rs:144-177`) and calls
`engine::execute_workflow`. Explicitly propagates `harness_instructions`
and `harness_memory_file`.

**execute** / **mcp** — stubs at `src/nodes/mod.rs:436-449`. Always returns
`EngineError::NodeExecutionFailed("{name} node type not yet implemented")`.

**custom registry** — `NodeRegistry` at `src/nodes/mod.rs:349-380`. Users
implement `CustomNode` (`src/nodes/mod.rs:330-342`) and register via
`EngineBuilder`. The `workflow` node type is itself registered this way
internally (`src/engine/builder.rs:287-288`).

---

## 2. Generate Node Pipeline (deep dive)

Source file: `src/nodes/generate.rs` (744 lines). This is the most important
node type; everything involving LLM calls flows through here.

### 2.1 Pipeline overview

The executor runs a strictly sequential pipeline, with branching only at the
final "execution path" step:

```
1. Extract provider + model             (generate.rs:91-115)
2. Resolve template name                (generate.rs:117-134)
   └─ Simple or Conditional (resolve_template_name, generate.rs:26-71)
3. Resolve variables from state         (generate.rs:137)
4. Build prompt                         (generate.rs:140-178)
   ├─ Render template.user_prompt + template.system_prompt via Handlebars
   ├─ compose_system_prompt(harness, skills, template_system)   <-- key
   ├─ Append "Respond in JSON format:\n{example}" if output_schema present
   └─ Fallback "Hello" if no template and no vars
5. Build image parts                    (generate.rs:188-196)
   └─ parse_image_input: data URI / https URL / raw base64 (magic-byte sniff)
6. Construct messages                   (generate.rs:181-217)
   └─ Multimodal user message when images present, otherwise plain text
7. Build ChatOptions with capability masking  (generate.rs:220-320)
8. Build tool definitions (if any)       (generate.rs:322-345)
9. Pick execution path                   (generate.rs:356-493)
   ├─ Streaming        (no tools, no json schema, handler present)
   ├─ Tool calling loop (tools present)
   └─ Simple chat
10. Parse outputs & schema-project      (generate.rs:495-581)
```

### 2.2 Variable resolution → template selection → prompt assembly

`resolve_template_name` at `generate.rs:26-71` walks `TemplateSelector`:

- `Simple(name)` → use directly.
- `Conditional { default, conditions }` → evaluates each `when` expression
  via `crate::expression::evaluate_expression` against the current state.
  First match wins; no match → `default`.

`super::resolve_variables` (defined in `src/nodes/mod.rs:40-66`) walks
`VariableBinding` entries:

- A source containing `.` is read as a state path.
- A source without `.` is a literal (`parse_literal` coerces to
  bool/int/float/string).
- First non-null wins; otherwise `binding.default_value()`; otherwise
  `Value::Null`.

Templates are rendered with Handlebars (`render_template` at
`generate.rs:604-629`). On render failure, it falls back to `{{var}}` simple
substitution. If no template is configured, the executor builds a prompt from
the variable map as `key: value\n...` (`generate.rs:154-158`).

JSON output schema appending: if `template.output_schema` is non-empty, the
user prompt is suffixed with `"\n\nRespond in JSON format:\n" +
generate_example_json(schema)` (`generate.rs:168-178`). Example generation
recursively handles nested `children` via `build_example_object`
(`generate.rs:650-676`).

### 2.3 `compose_system_prompt` — layered system prompt

```rust
// src/nodes/generate.rs:588-602
fn compose_system_prompt(
    harness: Option<&str>,
    skills: Option<&str>,
    template: Option<&str>,
) -> Option<String>
```

It receives three optional layers and joins any non-None parts with
`"\n\n---\n\n"`. Returns `None` only if all three are None.

Current caller state (`generate.rs:146-166`):

| Layer    | Source                                       | Current state |
|----------|----------------------------------------------|---------------|
| harness  | `config.harness_instructions.as_deref()`     | ACTIVE — resolved in `execute_workflow` (§3) |
| skills   | hardcoded `None`                             | PLACEHOLDER — reserved for Phase 2 Skill System |
| template | `template.system_prompt` (rendered)          | ACTIVE — unchanged behaviour |

Harness is injected even when there is no template
(`generate.rs:160-164`) — important when a workflow uses harness to provide
the entire system prompt.

### 2.4 ChatOptions with capability masking

The executor consults two registry-level capability sources
(`generate.rs:224-225`):

- `config.model_registry.get_capabilities(provider, model)` →
  per-model `json`, `function_calling`, `thinking`, `thinking_json_compat`,
  `vision`, `max_temperature`.
- `config.model_registry.get_sampling(provider)` →
  `supports_top_p`, `supports_top_k`, `allow_both_temp_and_top_p`.

Capability-aware clamping (`generate.rs:228-283`):

| Template field     | Behaviour |
|--------------------|-----------|
| `temperature`      | clamped to `max_temperature(provider, model)` |
| `max_tokens`       | passed through unconditionally |
| `top_p`            | dropped if provider lacks `supports_top_p`; further dropped if provider disallows `top_p + nonzero temperature` simultaneously |
| `top_k`            | dropped unless provider `supports_top_k` |
| `thinking_budget`  | dropped if model `thinking == "none"` |
| `reasoning_effort` | parsed `low/medium/high`; always passed if set |
| `enable_search`    | set verbatim (DashScope / Qwen web search) |

JSON output mode (`generate.rs:286-320`): only enabled if the model supports
`json`. If enabled alongside `thinking_budget`, the compatibility flag
`thinking_json_compat` decides: `None` means drop thinking and keep JSON;
`Full`/`Partial` keeps both. When unsupported, the executor relies solely on
the prompt-injected schema + robust extraction.

Tool definitions (`generate.rs:322-345`): only attached if
`function_calling` capability is true. Each `ToolRef` in `config.tools`
becomes a `ToolDefinition { name, description, parameters }` (defaults to
`{ "type":"object","properties":{} }` when parameters missing).

### 2.5 Execution path branching

Selector logic at `generate.rs:361-364`:

```
should_stream = node_config.stream
              && config.stream_handler.is_some()
              && !has_json_schema
              && !has_tools
```

Three mutually exclusive branches:

**A. Streaming** (`generate.rs:368-382`)
Calls `provider.stream(messages, options, callback)`. The callback forwards
text chunks to the registered `StreamHandler` via
`handler.on_text_chunk(node_id, chunk, accumulated)`. Returns
`(content, None, usage, ttft)`. `thinking` is never captured in this path.

**B. Tool calling loop** (`generate.rs:383-475`)
Multi-round loop up to `node_config.max_tool_rounds` (default 10):

1. `provider.chat(conversation.clone(), options.clone()).await`.
2. Accumulate `total_usage` (input/output/total).
3. If `response.tool_calls` is `Some` and non-empty:
   a. Push an assistant message with `content + tool_calls` to the
      conversation (`generate.rs:414-420`).
   b. For each call, look up the matching `ToolRef`, parse `arguments` as an
      object into a `HashMap`, execute the subgraph via
      `super::subgraph_runner::execute`, and serialize the subgraph output
      into JSON text. Errors become string messages.
   c. Append a tool result via
      `Message::tool_result_with_name(tc.id, tc.name, content)` —
      the `tool_name` field is required for Gemini's `functionResponse`.
   d. `continue` to next round.
4. If no tool calls → this is the final response; break with
   `final_content = response.content`, `final_thinking = response.thinking`.

Returns `(final_content, final_thinking, Some(total_usage), None)` (no TTFT
in this path).

**C. Simple chat** (`generate.rs:476-493`)
Single `provider.chat()` call. Returns `(content, thinking, Some(usage), None)`.

### 2.6 Output parsing and special fields

After the branch, the executor builds the `outputs: HashMap<String, Value>`.

`output` field (`generate.rs:498-539`):
- If `output_schema` is set and non-empty, it tries `serde_json::from_str`,
  falling back to `super::extract_json_from_content` (robust extraction with
  4 strategies: ```json fence, generic ``` fence, balanced `{ }`, balanced
  `[ ]` — see `src/nodes/mod.rs:100-187`). Parsed JSON is projected per
  schema field; missing fields fall back to `get_default_for_type`. On parse
  failure, raw content is written to `output`.
- If no schema, raw content is written to `output`.

Reserved underscore outputs written alongside:

| Field                       | Source | Condition |
|-----------------------------|--------|-----------|
| `output`                    | parsed response or raw content | always |
| `_thinking`                 | `response.thinking` (chat path) | present only when provider returned thinking text |
| `_ttft_ms`                  | streaming TTFT | streaming path only |
| `_usage`                    | `{ provider, model, input_tokens, output_tokens, [search_count] }` | when usage is available |
| `_prompt`                   | `final_prompt` (unrendered template debug) | always |
| `_actual_system_prompt`     | rendered system prompt as sent to LLM | only if `config.capture_actual_prompt` |
| `_actual_user_prompt`       | rendered user prompt as sent to LLM   | only if `config.capture_actual_prompt` |
| `_raw_response`             | raw content before parsing            | only if `config.capture_raw_response` |

The `_usage` field is special: the parent `execute_workflow` aggregates it
into `ExecutionResult.token_usage` (`src/engine/executor.rs:678-687`).

---

## 3. Harness — Current Implementation

Reconstructed from commit `2706b69`: "feat: harness instructions — workflow-level
system_prompt injection".

### 3.1 HarnessConfig schema

```rust
// src/schema/workflow.rs:225-238
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessConfig {
    #[serde(default)]
    pub instructions: Option<String>,   // inline text

    #[serde(default)]
    pub files: Vec<String>,             // external file paths (concatenated)

    #[serde(default)]
    pub memory_file: Option<String>,    // persistent memory file
}
```

Attached to the workflow via
`PreparedWorkflow.harness: Option<HarnessConfig>` at
`src/schema/workflow.rs:219-220`.

YAML shape:

```yaml
harness:
  instructions: |
    You are a translator. Always reply in Chinese...
  files:
    - ./prompts/guidelines.md
  memory_file: ./state/memory.json
```

### 3.2 Where it's resolved

`execute_workflow` runs a pre-pass at `src/engine/executor.rs:486-516`:

1. `harness.instructions` → pushed as the first part if present.
2. Each `harness.files[i]` → `std::fs::read_to_string` (blocking, sync);
   read failures are logged at `warn!` and silently skipped.
3. `harness.memory_file` → read if present and non-empty; wrapped with a
   `"## Memory\n"` prefix; **and** stored back into
   `config.harness_memory_file` for downstream (currently not written to
   after execution).
4. If any parts exist, joined with `"\n\n---\n\n"` and stored in
   `config.harness_instructions: Option<String>`.

Both fields live on `ExecutionConfig`
(`src/engine/executor.rs:231-237`):

```rust
/// Resolved harness instructions text (concatenated from inline + files + memory).
pub harness_instructions: Option<String>,
/// Harness memory file path (for post-execution update)
pub harness_memory_file: Option<String>,
```

Builder methods `with_harness_instructions` and `with_harness_memory_file`
exist at `src/engine/executor.rs:362-371` but are not used by the Engine
`run_*` methods — resolution happens inside `execute_workflow`.

### 3.3 How it reaches generate nodes

`GenerateExecutor` reads `config.harness_instructions` and passes it as the
first argument of `compose_system_prompt`
(`src/nodes/generate.rs:146-150`, `160-164`). See §2.3.

### 3.4 Sub-workflow propagation

`workflow_node.rs` explicitly copies both fields into the child
`ExecutionConfig` at `src/nodes/workflow_node.rs:170-171`:

```rust
harness_instructions: config.harness_instructions.clone(),
harness_memory_file: config.harness_memory_file.clone(),
```

This means a parent workflow's harness also binds the generate nodes inside
its sub-workflows.

### 3.5 What's done vs. not done

Done:

- Harness struct + schema parsing.
- `inline instructions` + `files` + `memory_file` read on startup.
- Concatenation with `---` separators.
- Propagation into `ExecutionConfig` and forwarding to sub-workflows.
- Injection into `compose_system_prompt` (first layer).
- Verification example: `examples/test_harness.rs` demonstrates the
  contents of `_actual_system_prompt` containing harness text. It compares
  two runs, one with `harness:`, one without.

NOT done:

- **Post-execution memory write-back**. `config.harness_memory_file` is
  stored but nothing writes to it after execution. Grep confirms only reads
  and clones of the field; no `std::fs::write` against it. This is the
  outstanding hole in the "CLAUDE.md-style persistent memory" story —
  currently memory is read-only.
- `skills` argument to `compose_system_prompt` is hardcoded to `None`
  (two call sites in `generate.rs:149` and `generate.rs:162`). Reserved for
  Phase 2 Skill System.
- File reads are **synchronous** `std::fs::read_to_string`. Fine for small
  text, but runs on the async executor thread — no `tokio::fs`.

---

## 4. Tool Calling Flow (provider-level)

Full details in `docs/dev-log-2026-04-09.md` (commit `1b150c7`). Summary:

### 4.1 Build path

`GenerateExecutor` converts `config.tools: Vec<ToolRef>` into
`Vec<ToolDefinition>` (defined at `src/providers/mod.rs:313-322`) and stores
them on `ChatOptions.tools: Option<Vec<ToolDefinition>>`
(`src/providers/mod.rs:362-363`). Capability gating via
`model_caps.function_calling`.

### 4.2 Per-provider serialization

| Provider  | Field name in request      | Schema key      | Location |
|-----------|----------------------------|-----------------|----------|
| OpenAI / OpenAI-compatible | native `tools`   | `parameters`    | `src/providers/openai.rs`, `openai_compatible.rs` |
| Anthropic | `tools`                     | `input_schema`  | `src/providers/anthropic.rs:164` |
| Gemini    | `tools.functionDeclarations`| `parameters`    | `src/providers/gemini.rs:105-116` |

Anthropic parses `tool_use` content blocks into `ToolCall` structs at
`src/providers/anthropic.rs:217-221`. Gemini parses `functionCall` parts. OpenAI
uses its native `tool_calls`.

### 4.3 Tool result feedback

Messages in `src/providers/mod.rs`:

- `Message::tool_result_with_name(tool_call_id, tool_name, content)` at
  `src/providers/mod.rs:226-234` — required because Gemini's
  `functionResponse` needs the tool name, not just the ID.
- Generate uses this exclusively (`src/nodes/generate.rs:462`).

Provider-specific content shaping lives in `src/providers/content.rs`:
Anthropic converts tool results into `tool_result` content blocks and
assistant tool calls into `tool_use` blocks; Gemini converts tool results
into `functionResponse` parts and assistant tool calls into `functionCall`
parts.

### 4.4 Multi-round loop

All orchestration lives in `src/nodes/generate.rs:383-475`. The loop:

1. LLM call → `response.tool_calls`.
2. If present, push assistant `tool_use` message, execute subgraphs, append
   tool result messages, continue.
3. If absent, break and return final response.

Round cap is `max_tool_rounds` (default 10). Tokens are accumulated across
rounds including tokens from subgraph LLM calls
(`src/nodes/generate.rs:447-449`).

---

## 5. Subgraphs and the Workflow Execution Model (brief)

### 5.1 Subgraph definition

```rust
// src/schema/workflow.rs:129-136
pub struct Subgraph {
    pub nodes: Vec<YamlNode>,
    pub edges: Vec<YamlEdge>,
}
```

Stored at `PreparedWorkflow.subgraphs: HashMap<String, Subgraph>` and merged
into `ExecutionConfig.subgraphs` at workflow setup
(`src/engine/executor.rs:477-479`). Referenced by:

- `ToolRef.subgraph_ref` for supervisor + generate tool calling.
- `IteratorExecutor.config.subgraph` for per-item subgraph execution.

### 5.2 `subgraph_runner::execute`

`src/nodes/subgraph_runner.rs:32-176`. Runs a mini-workflow with isolated
state:

- Creates a fresh `ExecutionState::new(input)` (no access to parent state).
- Builds incoming edge map, then loops: find nodes with satisfied inputs
  (via variable binding check or fallback to edge check), execute via
  `create_executor(&node.node_type)`, write outputs back to local state,
  track per-node `SubgraphNodeExecution` records.
- Final output is the last response node's outputs if present, otherwise
  the last executed node's outputs.
- Returns `SubgraphRunResult { outputs, usage, executions }`.

The runner is shared between `generate` tool calls, `supervisor` tool
execution (`src/nodes/supervisor.rs:681` — `execute_subgraph`), and
`iterator` per-item execution.

### 5.3 Tool-call → subgraph dispatch in generate

At `src/nodes/generate.rs:445-456`:

```rust
if let Some(subgraph) = config.subgraphs.get(&tool.subgraph_ref) {
    match super::subgraph_runner::execute(subgraph, input, config).await {
        Ok(result) => { /* accumulate usage + serialize outputs */ }
        Err(e)    => format!("Tool execution error: {}", e),
    }
}
```

Tool-call arguments (a JSON object) become the subgraph's input `HashMap`.
Subgraph outputs (the response node's outputs, or the last node's) are
`serde_json::to_string`'d and sent back as the tool result text.

### 5.4 Main execution loop (input-based dependency checking)

The high-level loop in `execute_workflow`
(`src/engine/executor.rs:572-764`):

1. **Timeout check** — per-loop iteration, returns `Timeout` result with
   pending/in-flight node diagnostics.
2. **Ready set** — `pending_nodes.filter(|id| graph.inputs_satisfied(id,
   state))`. Nodes become ready when every configured variable binding can
   be resolved AND at least one incoming edge is "active" (executed source,
   non-dead, condition passed). Source of truth:
   `ExecutionGraph::inputs_satisfied` at `src/engine/graph.rs:113-218`.
3. **Parallel execution** — `execute_nodes_parallel`
   (`executor.rs:808-856`) runs all ready nodes in parallel via
   `futures::future::join_all`. Each call dispatches to
   `execute_single_node` → `execute_node_once` with retry/timeout handling
   per `config.error_handling`.
4. **Write results** — outputs are written to shared `ExecutionState`
   under a write lock. Token usage accumulates. Checkpoint nodes cause an
   immediate `Paused` return. Response node outputs are captured as
   `final_output`.
5. **Edge evaluation** — `evaluate_edges` (`executor.rs:1033-1196`)
   handles exit edges (from Supervisor), on_error edges, default edges,
   conditional edges, and `max_iterations` caps. Dead conditional targets
   get marked via `state.mark_edge_dead`.
6. **Dead path propagation** — `propagate_dead_paths`
   (`executor.rs:1205-1244`). Fixed-point walk: a non-start node is dead
   when all incoming edges are inactive (source dead OR edge dead). Dead
   nodes get removed from `pending_nodes` so they never execute.
7. **Terminate** — when `pending_nodes` is empty and `in_flight` is empty.

---

## 6. ExecutionConfig Field Map

Defined at `src/engine/executor.rs:174-238`. Grouped by lifetime.

### 6.1 Engine-level (shared across all runs)

| Field                 | Type                                      | Purpose |
|-----------------------|-------------------------------------------|---------|
| `providers`           | `ProviderRegistry`                        | LLM provider clients keyed by name |
| `embedding_providers` | `EmbeddingRegistry`                       | Embedding provider clients |
| `search_providers`    | `SearchProviderRegistry`                  | Web search provider clients (Tavily etc.) |
| `node_registry`       | `Arc<NodeRegistry>`                       | Custom node trait registry (`workflow`, user nodes) |
| `pools`               | `Arc<ConnectionPools>`                    | PostgreSQL / Redis connection pools |
| `workflows`           | `Arc<HashMap<String, PreparedWorkflow>>`  | All loaded workflows (for sub-workflow lookup) |
| `model_registry`      | `Arc<ModelRegistry>`                      | Per-model capability flags (json, vision, thinking, temperature caps, sampling) |
| `transforms`          | `HashMap<String, String>`                 | Engine-wide Rhai transform scripts |

### 6.2 Workflow-level (per workflow)

| Field           | Type                                     | Purpose |
|-----------------|------------------------------------------|---------|
| `templates`     | `HashMap<String, Template>`              | Templates merged from workflow + engine |
| `subgraphs`     | `HashMap<String, Subgraph>`              | Subgraphs for supervisor/generate tools and iterator |
| `data_sources`  | `HashMap<String, DataSourceDef>`         | Data source definitions for data_connector nodes |

### 6.3 Run-level options (per execution)

| Field                     | Type                                | Purpose |
|---------------------------|-------------------------------------|---------|
| `timeout`                 | `Duration`                          | Global workflow timeout (default 300s) |
| `max_iterations`          | `u32`                               | Default per-edge iteration cap (default 100) |
| `capture_actual_prompt`   | `bool`                              | Record `_actual_system_prompt`/`_actual_user_prompt` |
| `capture_raw_response`    | `bool`                              | Record `_raw_response` before schema parsing |
| `stream_handler`          | `Option<Arc<dyn StreamHandler>>`    | Optional sink for streaming text chunks |

### 6.4 Harness (workflow-level, resolved per execution)

| Field                   | Type               | Purpose |
|-------------------------|--------------------|---------|
| `harness_instructions`  | `Option<String>`   | Pre-resolved text (inline + files + memory joined with `---`); read by `GenerateExecutor` |
| `harness_memory_file`   | `Option<String>`   | Path to memory file. Currently **read-only** — no post-execution write-back. |

---

## 7. What's NOT in the Engine Yet

Cross-referenced against the roadmap context in `engine_todo.md` and
`harness-design.md`. This is a forward-pointing summary, not a design.

- **MCP node** (`NodeType::Mcp`) — still `NotImplementedExecutor("mcp")` at
  `src/nodes/mod.rs:413`. Planned Phase 4. Design doc: `docs/MCP_DESIGN.md`.
- **Execute node** (`NodeType::Execute`) — code-execution node. Still a
  `NotImplementedExecutor("execute")` placeholder. Planned P1 Code Node.
- **Skill System** (Phase 2 / harness-design.md §3) — the `skills` parameter
  of `compose_system_prompt(harness, skills, template)` is hardcoded to
  `None`. No skill discovery, loading, or dispatch yet.
- **Harness post-execution memory write-back** — `harness_memory_file` is
  read at workflow start but nothing writes to it after execution. The
  "CLAUDE.md persistent memory" story is half-complete. This is the single
  most actionable harness gap; see §3.5.
- **Context Management** (Phase 3 / harness-design.md §4) — not yet
  implemented. Conversation / context window management lives entirely in
  the tool-calling loop right now and has no compaction, persistence, or
  summarization hooks.
- **DryRun / validation mode** — no flag exists to execute a workflow
  without side effects; `ExecutionConfig` has no such field.
- **Generate node enhancements** (per feedback memo) — explicitly deferred
  until MCP/skill/tool calling are done together.
- **Anthropic + Gemini extra tool-calling edge cases** — basic path shipped
  `1b150c7`; follow-ups for streaming tool calls and parallel tool call
  batching per dev-log remarks.
- **Multi-turn memory / ES / SQLite / Kafka / Trigger** — all P2/P3 items;
  no code in the tree yet.

---

*Snapshot produced 2026-04-10. If anything here drifts from the code, trust
the code.*
