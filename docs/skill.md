# rp-engine Skill Guide

## Overview

rp-engine is a YAML-native agent workflow engine written in Rust. You define workflows as YAML files containing nodes, edges, templates, and configuration. The engine parses these definitions, resolves dependencies, and executes nodes in the correct order -- making LLM calls, running scripts, calling MCP tools, iterating over data, and orchestrating multi-agent plans.

### Core Concepts

| Concept | What it is |
|---|---|
| **Node** | A unit of work: `start`, `generate`, `response`, `checkpoint`, `transform`, `iterator`, `supervisor`, `workflow`, `data_connector`, `skill_set`, `mcp`, `mcp_tools` |
| **Edge** | A connection from one node to another, optionally with a `condition`, `max_iterations` (loop cap), `exit_reason` (supervisor routing), or `on_error` flag |
| **Template** | A prompt definition with `user_prompt`, optional `system_prompt`, LLM config (`temperature`, `max_tokens`, etc.), and optional `output_schema` for structured JSON output |
| **Variable binding** | Maps a variable name to a state path (`node_id.field`), a literal, or multiple fallback sources |
| **Subgraph** | A mini-workflow (nodes + edges) used as a tool by `supervisor` and `iterator` nodes |
| **Data source** | A connection config for external data (PostgreSQL, Redis, S3, REST API, etc.) |
| **Provider** | An LLM provider (qwen, openai, anthropic, gemini, deepseek, xai, zhipu, moonshot, minimax, huoshan, openai_compatible) |
| **Harness** | Workflow-level instructions injected into every generate node's system_prompt (like CLAUDE.md). Supports inline text, external files, and persistent memory. |
| **Skill** | Progressive disclosure knowledge bundles defined in `skill_set` nodes. LLM loads on demand via `load_skill` tool call. Three levels: L1 metadata, L2 full content, L3 references. |
| **MCP** | Model Context Protocol integration. `mcp` nodes call a specific tool on an MCP server. `mcp_tools` nodes discover server tools for LLM-driven tool calling via `mcp_tool_sets` on generate nodes. |

### State Model

Every node writes its outputs to the execution state under its own ID. Downstream nodes read values using **paths** like `node_id.field_name`. The special prefix `input.` refers to the original workflow input.

---

## Quick Start

A minimal workflow has three nodes: `start` receives input, `generate` calls an LLM, and `response` returns output.

```yaml
version: "1.0"
name: "hello_world"

nodes:
  - id: start
    type: start

  - id: greet
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_greet
      variables:
        name: "start.name"

  - id: response
    type: response
    config:
      output:
        greeting: "greet.output"

edges:
  - from: start
    to: greet
  - from: greet
    to: response

templates:
  tpl_greet:
    user_prompt: "Say hello to {{name}} in a friendly way."
    config:
      temperature: 0.7
      max_tokens: 100
```

Run with input `{"name": "Alice"}` and get back `{"greeting": "Hello Alice! ..."}`.

---

## Node Types

### start

The entry point of every workflow. Exactly one `start` node is required.

**Purpose:** Receives workflow input and optionally remaps fields for downstream nodes.

**Passthrough mode** (no output config) -- all input fields are forwarded as-is:

```yaml
- id: start
  type: start
```

If the workflow is called with `{"user_query": "hello", "user_id": "u123"}`, downstream nodes can read `start.user_query` and `start.user_id` directly.

**With output mapping** -- selectively remap input fields:

```yaml
- id: start
  type: start
  config:
    output:
      query: "input.user_query"
      uid: "input.user_id"
```

Now downstream nodes read `start.query` and `start.uid`. The path `input.user_query` reads from the raw workflow input.

---

### generate -- LLM Call

The workhorse node. Calls an LLM provider with a rendered prompt template and returns the response.

**Required config:** `provider`, `model`, `template` (or use variables-as-prompt fallback).

#### Basic usage with template

```yaml
nodes:
  - id: think
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_think
      variables:
        question: "start.question"

templates:
  tpl_think:
    user_prompt: "Answer this question: {{question}}"
    system_prompt: "You are a helpful assistant."
    config:
      temperature: 0.7
      max_tokens: 512
```

Output: `think.output` contains the raw LLM response text.

#### Variable bindings -- all 3 forms

**Simple** -- single state path:

```yaml
variables:
  query: "start.user_query"
```

**Multi-source** -- first available wins (useful for optional upstream nodes):

```yaml
variables:
  context: ["search_node.results", "fallback_node.output"]
```

The engine tries `search_node.results` first. If that node did not run or the field is null, it falls back to `fallback_node.output`.

**With default** -- provides a fallback value when no source resolves:

```yaml
variables:
  language:
    from: "start.language"
    default: "en"
  max_items:
    from: ["config_node.limit", "start.limit"]
    default: 10
```

The `from` field accepts a single string or an array. If all sources are null, the `default` value is used. Without a default, the variable resolves to null.

Literal values (no dot in the path) are parsed with smart type detection:
- `"true"` / `"false"` become booleans
- `"42"` becomes a number
- `"3.14"` becomes a float
- Anything else stays a string

#### Output schema -- flat

Force the LLM to respond in structured JSON:

```yaml
templates:
  tpl_classify:
    user_prompt: "Classify this text: {{text}}"
    config:
      temperature: 0.3
      max_tokens: 200
    output_schema:
      - name: category
        type: string
        description: "The category of the text"
      - name: confidence
        type: number
        example: "0.95"
      - name: tags
        type: string[]
```

Each field in `output_schema` becomes a separate output. Downstream nodes access them as `node_id.category`, `node_id.confidence`, `node_id.tags`.

Supported types: `string`, `number`, `boolean`, `string[]`, `number[]`, `object`, `object[]`.

#### Output schema -- nested with children

For complex structures, use `children` on `object` or `object[]` fields:

```yaml
templates:
  tpl_plan:
    user_prompt: "Create a travel plan for {{destination}}."
    config:
      temperature: 0.7
      max_tokens: 2048
    output_schema:
      - name: title
        type: string
      - name: days
        type: object[]
        children:
          - name: day_number
            type: number
            example: "1"
          - name: activities
            type: string[]
          - name: hotel
            type: object
            children:
              - name: name
                type: string
              - name: price
                type: number
```

The engine generates an example JSON from the schema and appends it to the prompt. If the model supports JSON output mode, it is enabled automatically.

#### Streaming

Enable real-time token delivery for a generate node:

```yaml
- id: chat
  type: generate
  config:
    provider: qwen
    model: qwen-plus
    template: tpl_chat
    stream: true
    variables:
      message: "start.message"
```

Streaming is automatically disabled when:
- `output_schema` is present (JSON mode requires a complete response)
- `tools` are configured (tool calling needs the full response)
- No `StreamHandler` is registered in the engine

When streaming is active, the engine calls `StreamHandler.on_text_chunk(node_id, chunk, accumulated)` for each token. The output field `_ttft_ms` records the time-to-first-token in milliseconds.

#### Multimodal (images)

Send images alongside text to vision-capable models:

```yaml
- id: describe
  type: generate
  config:
    provider: qwen
    model: qwen-vl-plus
    template: tpl_describe
    images:
      - "start.photo_base64"
      - "start.reference_url"
    variables:
      instruction: "start.instruction"

templates:
  tpl_describe:
    user_prompt: "{{instruction}}"
    config:
      temperature: 0.5
      max_tokens: 1024
```

The `images` field is an array of variable bindings. Each resolved value can be:
- A **data URI**: `data:image/jpeg;base64,/9j/4AAQ...`
- An **HTTP URL**: `https://example.com/photo.jpg`
- **Pure base64** data: the engine detects the media type (JPEG, PNG, GIF, WebP) from magic bytes

Images are added as content parts in the user message alongside the text prompt.

#### Tool calling (tools + subgraphs + max_tool_rounds)

Enable native function calling with LLM-driven tool use:

```yaml
nodes:
  - id: agent
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_agent
      max_tool_rounds: 5
      tools:
        - name: get_weather
          description: "Get current weather for a city"
          ref: sg_weather
          parameters:
            type: object
            properties:
              city:
                type: string
                description: "City name"
            required: ["city"]
        - name: search_docs
          description: "Search internal documents"
          ref: sg_search
      variables:
        user_query: "start.query"

subgraphs:
  sg_weather:
    nodes:
      - id: fetch
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: tpl_weather
          variables: {}
    edges: []

  sg_search:
    nodes:
      - id: lookup
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: tpl_search
          variables: {}
    edges: []

templates:
  tpl_agent:
    user_prompt: "Help the user: {{user_query}}"
    system_prompt: "You are a helpful assistant with access to tools."
    config:
      temperature: 0.7
      max_tokens: 2048
  tpl_weather:
    user_prompt: "Return mock weather for the requested city based on: {{city}}"
    output_schema:
      - name: weather
        type: string
  tpl_search:
    user_prompt: "Search for: {{instruction}}"
    output_schema:
      - name: results
        type: string
```

The tool calling loop:
1. Send the prompt + tool definitions to the LLM
2. If the LLM returns tool calls, execute each tool's subgraph with the call arguments as input
3. Feed tool results back to the LLM as tool result messages
4. Repeat until the LLM responds with a final text answer (no tool calls) or `max_tool_rounds` is reached

Default `max_tool_rounds` is 10. Tool definitions are only sent if the model supports function calling.

#### Conditional template selection

Dynamically choose a template based on runtime state:

```yaml
- id: writer
  type: generate
  config:
    provider: qwen
    model: qwen-plus
    template:
      default: tpl_normal
      conditions:
        - when: "classifier.category == 'technical'"
          use: tpl_technical
        - when: "classifier.category == 'creative'"
          use: tpl_creative
    variables:
      topic: "start.topic"
```

Conditions are evaluated top-to-bottom. The first match wins. If no condition matches, `default` is used. The `when` field uses the same expression syntax as edge conditions.

#### Error handling (retry, timeout, on_exhausted)

Configure per-node retry and timeout behavior:

```yaml
- id: unreliable_call
  type: generate
  config:
    provider: qwen
    model: qwen-plus
    template: tpl_call
    error_handling:
      retry: 3
      retry_delay_ms: 2000
      timeout_ms: 30000
      on_exhausted: continue
    variables:
      prompt: "start.prompt"
```

| Field | Default | Description |
|---|---|---|
| `retry` | `0` | Number of retry attempts after failure |
| `retry_delay_ms` | `1000` | Delay between retries in milliseconds |
| `timeout_ms` | none | Per-node timeout; triggers retry or failure |
| `on_exhausted` | `"fail"` | `"fail"` = propagate error and terminate workflow. `"continue"` = store error in outputs and continue via `on_error` edges |

Using `on_exhausted: continue` with an `on_error` edge:

```yaml
edges:
  - from: unreliable_call
    to: fallback_node
    on_error: true
  - from: unreliable_call
    to: next_node
```

When `unreliable_call` fails after all retries, the `on_error: true` edge fires (to `fallback_node`). Normal edges are suppressed when an error edge activates.

#### Capability masking

The engine automatically adapts LLM requests to each model's capabilities:

- **Temperature:** Clamped to the model/provider maximum
- **top_p / top_k:** Silently dropped if the provider does not support them
- **JSON output mode:** Falls back to prompt-based JSON instruction if the model lacks JSON mode support
- **Thinking budget:** Dropped if the model does not support extended thinking. When thinking + JSON are incompatible, JSON takes priority and thinking is disabled
- **Function calling / tools:** Tool definitions are dropped with a warning if the model lacks function calling support

This means the same workflow YAML can target different models without modification.

#### All output fields

Every generate node produces these output fields:

| Field | Type | Always present | Description |
|---|---|---|---|
| `output` | string | Yes (when no schema) | Raw LLM response text. Not present when `output_schema` maps fields individually |
| *(schema fields)* | varies | Yes (when schema defined) | One field per `output_schema` entry, e.g. `node.answer`, `node.confidence` |
| `_usage` | object | Yes | `{"provider": "qwen", "model": "qwen-plus", "input_tokens": 150, "output_tokens": 42}`. May include `search_count` for web-search-enabled providers |
| `_prompt` | string | Yes | The final rendered user prompt sent to the LLM |
| `_thinking` | string | When model supports thinking | Extended thinking / chain-of-thought content from the model |
| `_ttft_ms` | number | Streaming only | Time to first token in milliseconds |
| `_actual_system_prompt` | string | When `capture_actual_prompt` enabled | The rendered system prompt |
| `_actual_user_prompt` | string | When `capture_actual_prompt` enabled | The rendered user prompt (same as `_prompt`) |
| `_raw_response` | string | When `capture_raw_response` enabled | The raw LLM response string before JSON parsing |

#### Complete working example -- structured output with system prompt

```yaml
version: "1.0"
name: "sentiment_analyzer"

nodes:
  - id: start
    type: start

  - id: analyze
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_sentiment
      variables:
        text: "start.text"

  - id: response
    type: response
    config:
      output:
        sentiment: "analyze.sentiment"
        score: "analyze.score"
        reasoning: "analyze.reasoning"

edges:
  - from: start
    to: analyze
  - from: analyze
    to: response

templates:
  tpl_sentiment:
    system_prompt: "You are a sentiment analysis expert. Analyze text precisely."
    user_prompt: |
      Analyze the sentiment of the following text:

      {{text}}
    config:
      temperature: 0.3
      max_tokens: 512
    output_schema:
      - name: sentiment
        type: string
        description: "positive, negative, or neutral"
      - name: score
        type: number
        description: "Confidence score from 0.0 to 1.0"
        example: "0.85"
      - name: reasoning
        type: string
        description: "Brief explanation of the sentiment classification"
```

---

### response

The terminal node of every workflow. At least one `response` node is required.

**Purpose:** Collects outputs from upstream nodes and returns them as the workflow result.

```yaml
- id: response
  type: response
  config:
    output:
      answer: "generator.answer"
      sources: "search.results"
      tokens_used: "generator._usage"
```

Each key in `output` becomes a field in the workflow's return value. The value is a state path (`node_id.field`). If the path does not resolve, the field is `null`.

---

### checkpoint

Pauses workflow execution and returns the current state for later resume.

**Purpose:** Implement human-in-the-loop patterns, long-running workflows, or multi-step interactions.

```yaml
nodes:
  - id: start
    type: start

  - id: draft
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_draft
      variables:
        topic: "start.topic"

  - id: review_checkpoint
    type: checkpoint
    config:
      output:
        draft_text: "draft.output"
        needs_review: "draft.needs_review"

  - id: finalize
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_finalize
      variables:
        draft: "draft.output"
        feedback: "input.feedback"

  - id: response
    type: response
    config:
      output:
        final_text: "finalize.output"

edges:
  - from: start
    to: draft
  - from: draft
    to: review_checkpoint
  - from: review_checkpoint
    to: finalize
  - from: finalize
    to: response
```

When execution reaches `review_checkpoint`:
1. The engine pauses and returns a `CheckpointState` containing `draft_text` and `needs_review`
2. The caller collects human feedback
3. The caller resumes the workflow by passing the checkpoint state plus new input (e.g., `{"feedback": "Please add more detail"}`)
4. Execution continues from `finalize` using the resumed state

The checkpoint node config uses the same `output` mapping as the response node.

---

### transform -- Rhai Script

Runs a [Rhai](https://rhai.rs) script for pure computation -- no LLM calls, no I/O.

> **Rhai is not Rust.** While Rhai syntax looks similar to Rust, it is a separate scripting language. Key differences: no turbofish (`::< >`), no `.collect()`, no trait methods, no ownership/borrowing. See [Rhai documentation](https://rhai.rs/book/) for supported syntax. Built-in functions: `now()`, `uuid()`, `json_encode(value)`, `unique(array)`.

**Purpose:** Data transformation, aggregation, formatting, filtering.

#### Inline script

```yaml
- id: prepare
  type: transform
  config:
    script: |
      let items = raw_items;
      let filtered = items.filter(|item| item.score > 0.5);
      #{
        "top_items": filtered,
        "count": filtered.len(),
        "timestamp": now()
      }
    variables:
      raw_items: "search.results"
```

#### File reference

Reference a named script from the workflow's `transforms` section or the engine's transforms directory:

```yaml
- id: format
  type: transform
  config:
    file: format_results
    variables:
      data: "generator.output"
      max_length: "start.max_length"

transforms:
  format_results: |
    let text = data;
    if text.len() > max_length {
      text.truncate(max_length);
      text += "..."
    }
    #{
      "formatted": text,
      "was_truncated": text.len() > max_length
    }
```

When both `script` and `file` are present, `script` takes precedence.

#### Built-in functions

| Function | Returns | Description |
|---|---|---|
| `now()` | string | Current timestamp in Beijing time (UTC+8), format: `"2026-04-05 14:30:00"` |
| `uuid()` | string | Random UUID v4, e.g. `"f47ac10b-58cc-4372-a567-0e02b2c3d479"` |
| `json_encode(value)` | string | Serialize any value to a JSON string |
| `unique(array)` | array | Deduplicate array elements, preserving order |

#### Returning map vs single value

**Map return** -- keys become individual output fields:

```yaml
script: |
  #{
    "greeting": "Hello, " + name,
    "id": uuid()
  }
```

Downstream: `transform_node.greeting`, `transform_node.id`

**Single value return** -- wrapped as `result`:

```yaml
script: "40 + 2"
```

Downstream: `transform_node.result` (value: `42`)

#### Variable injection

All resolved variables are injected into the Rhai scope as named variables. Additionally, a `_vars` map containing all variables is always available:

```yaml
- id: merge
  type: transform
  config:
    script: |
      // Individual variables are accessible directly
      let msg = "Hello, " + name;
      // _vars map contains everything
      let all_json = json_encode(_vars);
      #{
        "message": msg,
        "debug_vars": all_json
      }
    variables:
      name: "start.user_name"
      count: "start.item_count"
```

#### Safety limits

The Rhai engine enforces these limits to prevent runaway scripts:

| Limit | Value |
|---|---|
| Max operations | 100,000 |
| Max string size | 1 MB |
| Max array size | 10,000 elements |
| Max map size | 10,000 entries |

Scripts exceeding these limits produce a `TransformError`.

#### Complete working example

```yaml
version: "1.0"
name: "data_pipeline"

nodes:
  - id: start
    type: start

  - id: analyze
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_analyze
      variables:
        data: "start.raw_data"

  - id: post_process
    type: transform
    config:
      script: |
        let items = results;
        let deduped = unique(items);
        let enriched = [];
        for (index, item) in deduped {
          let entry = #{
            "index": index,
            "value": item,
            "processed_at": now()
          };
          enriched.push(entry);
        }
        #{
          "items": enriched,
          "total": enriched.len(),
          "run_id": uuid()
        }
      variables:
        results: "analyze.items"

  - id: response
    type: response
    config:
      output:
        items: "post_process.items"
        total: "post_process.total"
        run_id: "post_process.run_id"

edges:
  - from: start
    to: analyze
  - from: analyze
    to: post_process
  - from: post_process
    to: response

templates:
  tpl_analyze:
    user_prompt: "Extract key items from this data: {{data}}"
    config:
      temperature: 0.5
      max_tokens: 1024
    output_schema:
      - name: items
        type: string[]
```

---

### search -- _(Removed)_

Search node type has been removed. Search capability will be available as a built-in tool for generate nodes in a future release. YAML files using `type: search` will fail with `CustomNodeNotFound`.

---

### mcp -- Explicit MCP Tool Call

Calls a specific tool on an MCP server using Streamable HTTP transport (JSON-RPC 2.0).

#### Configuration

| Field | Required | Description |
|---|---|---|
| `server` | Yes | MCP server ID from `mcp_servers` top-level section |
| `tool` | Yes | Tool name to invoke |
| `variables` | No | Tool arguments (variable bindings) |

#### Output fields

| Field | Type | Description |
|---|---|---|
| `result` | any | Tool call result from MCP server |
| `_timing_ms` | number | Execution time in milliseconds |
| `_error` | bool | Present and `true` on failure |
| `_error_message` | string | Error description (on failure) |

#### Example

```yaml
mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"

nodes:
  - id: start
    type: start

  - id: wiki
    type: mcp
    config:
      server: deepwiki
      tool: read_wiki_structure
      variables:
        repoName: "start.repo"

  - id: response
    type: response
    config:
      output:
        structure: "wiki.result"

edges:
  - from: start
    to: wiki
  - from: wiki
    to: response
```

---

### mcp_tools -- MCP Tool Discovery for LLM

Connects to an MCP server, discovers available tools via `tools/list`, and outputs tool definitions to state. Generate nodes consume these tools via `mcp_tool_sets`.

#### Configuration

| Field | Required | Description |
|---|---|---|
| `server` | Yes | MCP server ID from `mcp_servers` top-level section |

#### Output fields

| Field | Type | Description |
|---|---|---|
| `tools` | array | Array of MCP tool definitions (name, description, inputSchema) |
| `server` | string | Server ID (for routing tool calls) |
| `_tool_count` | number | Number of tools discovered |

#### mcp_tools + generate integration

```yaml
mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"

nodes:
  - id: start
    type: start

  - id: wiki_tools
    type: mcp_tools
    config:
      server: deepwiki

  - id: agent
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: agent_tpl
      mcp_tool_sets: ["wiki_tools"]
      variables:
        question: "start.question"

  - id: response
    type: response
    config:
      output:
        answer: "agent.output"

edges:
  - from: start
    to: wiki_tools
  - from: wiki_tools
    to: agent
  - from: agent
    to: response

templates:
  agent_tpl:
    user_prompt: "{{question}}"
    system_prompt: "You are a helpful assistant with access to wiki tools."
    config:
      temperature: 0.7
      max_tokens: 4096
```

The LLM sees MCP tools alongside any user-defined subgraph tools and skill tools. During the tool calling loop, MCP tool calls are dispatched to the corresponding MCP server.

---

### iterator -- Batch Processing

Processes an array by running a subgraph for each element.

**Purpose:** Fan-out over a list of items -- translate N paragraphs, analyze N images, enrich N records.

#### Configuration

| Field | Default | Description |
|---|---|---|
| `items` | (required) | Variable binding to the array to iterate over |
| `subgraph` | (required) | Reference to a subgraph ID |
| `item_var` | `"item"` | Variable name for the current item inside the subgraph |
| `index_var` | none | Variable name for the 0-based index |
| `parallel` | `false` | Run iterations concurrently |
| `max_concurrency` | `5` | Max parallel iterations (when `parallel: true`) |
| `max_iterations` | `1000` | Safety cap on array length |
| `on_error` | `"fail_fast"` | `"fail_fast"` = abort on first error. `"collect"` = continue and collect errors |

#### Shared variables

Variables defined in `config.variables` are passed to every iteration alongside `item_var` and `index_var`:

```yaml
- id: translate_all
  type: iterator
  config:
    items: "splitter.paragraphs"
    subgraph: sg_translate
    item_var: paragraph
    index_var: idx
    parallel: true
    max_concurrency: 3
    on_error: collect
    variables:
      target_language: "start.language"
```

Inside the subgraph, the start node receives `paragraph` (current item), `idx` (0-based index), and `target_language` (shared).

#### Output fields

| Field | Type | Description |
|---|---|---|
| `results` | array | Array of subgraph outputs, preserving input order. Failed items contain `{"_error": "message"}` |
| `count` | number | Total number of items processed |
| `success_count` | number | Number of successful iterations |
| `error_count` | number | Number of failed iterations |
| `errors` | array | (Only when errors exist) `[{"index": 2, "error": "..."}]` |
| `_usage` | object | `{"input_tokens": N, "output_tokens": N}` aggregated across all iterations |

#### Complete working example with subgraph

```yaml
version: "1.0"
name: "batch_translator"

nodes:
  - id: start
    type: start

  - id: split
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_split
      variables:
        text: "start.document"

  - id: translate_all
    type: iterator
    config:
      items: "split.paragraphs"
      subgraph: sg_translate
      item_var: paragraph
      index_var: idx
      parallel: true
      max_concurrency: 3
      on_error: collect
      variables:
        target_lang: "start.target_language"

  - id: combine
    type: transform
    config:
      script: |
        let parts = results;
        let combined = "";
        for (i, part) in parts {
          if i > 0 { combined += "\n\n"; }
          let text = part.translated_text;
          if text == () { text = "[translation failed]"; }
          combined += text;
        }
        #{
          "full_translation": combined,
          "paragraph_count": parts.len()
        }
      variables:
        results: "translate_all.results"

  - id: response
    type: response
    config:
      output:
        translation: "combine.full_translation"
        paragraph_count: "combine.paragraph_count"
        success_count: "translate_all.success_count"
        error_count: "translate_all.error_count"

edges:
  - from: start
    to: split
  - from: split
    to: translate_all
  - from: translate_all
    to: combine
  - from: combine
    to: response

subgraphs:
  sg_translate:
    nodes:
      - id: do_translate
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: tpl_translate
          variables:
            paragraph: "input.paragraph"
            idx: "input.idx"
            target_lang: "input.target_lang"
    edges: []

templates:
  tpl_split:
    user_prompt: "Split this document into paragraphs: {{text}}"
    config:
      temperature: 0.3
      max_tokens: 4096
    output_schema:
      - name: paragraphs
        type: string[]

  tpl_translate:
    user_prompt: |
      Translate the following paragraph to {{target_lang}}.
      Paragraph #{{idx}}:

      {{paragraph}}
    config:
      temperature: 0.3
      max_tokens: 1024
    output_schema:
      - name: translated_text
        type: string
```

Note: Inside the subgraph, input variables are accessed via `input.paragraph`, `input.idx`, etc., because the iterator creates a fresh execution state with those values as the workflow input.

---

### supervisor -- Multi-Agent

Implements a planning loop: an LLM Planner generates a list of actions, and the engine executes each action as a subgraph (tool).

#### How it works

1. **Plan:** The Planner LLM is called once with the user request + available tools. It returns a JSON `todos` array.
2. **Execute:** The engine processes each todo item. Dependencies between actions are respected via `action_prerequest`.
3. **Exit:** The loop exits when all todos are complete (`todo_empty`), or an exit condition triggers (`timeout`, `max_iterations`, `max_result_length`, `tool_failures`).

#### Configuration

| Field | Default | Description |
|---|---|---|
| `template` | (required) | Planner prompt template |
| `provider` | (required) | LLM provider for the Planner |
| `model` | (required) | LLM model for the Planner |
| `tools` | (required) | Array of tool references |
| `max_iterations` | `10` | Max planning/execution iterations |
| `max_retry_per_tool` | `3` | Max retry attempts per tool execution |
| `parallel` | `false` | Execute independent tools in parallel |
| `timeout` | `300000` | Timeout in milliseconds (default: 5 min) |
| `max_result_length` | `1000000` | Max total result chars |
| `fail_on_tool_count` | none | Exit after N tool failures |
| `summarizer_template` | none | Optional post-processing template |

#### Tools and subgraph references

Each tool maps a name to a subgraph:

```yaml
tools:
  - name: web_search
    description: "Search the web for information"
    ref: sg_web_search
  - name: sql_query
    description: "Execute a SQL query against the database"
    ref: sg_sql_query
    parameters:
      type: object
      properties:
        query:
          type: string
          description: "The SQL query to execute"
      required: ["query"]
```

The `parameters` field is optional. If provided, it gives the Planner LLM structured information about what the tool accepts.

#### Exit conditions and edge routing

Supervisor exit reasons can route to different downstream nodes:

```yaml
edges:
  - from: supervisor
    to: success_handler
    exit_reason: todo_empty
  - from: supervisor
    to: timeout_handler
    exit_reason: timeout
  - from: supervisor
    to: fallback
    exit_reason: default        # catches any exit reason
```

Available exit reasons: `todo_empty`, `timeout`, `max_iterations`, `max_result_length`, `tool_failures`, `default`.

#### Output fields

| Field | Type | Description |
|---|---|---|
| `task_result` | object | Full execution report: `{summary, planner_usage, planner_timing_ms, actions, stats}` |
| `exit_reason` | string | Why the supervisor exited: `"todo_empty"`, `"timeout"`, etc. |
| `_usage` | object | `{"input_tokens": N, "output_tokens": N}` aggregated total |
| `action_a`, `action_b`, ... | object | Individual action outputs keyed by short IDs (a-z, aa-az, etc.) |

#### Complete working example

```yaml
version: "1.0"
name: "task_planner"

input:
  fields:
    - name: user_input
      type: string
      required: true

nodes:
  - id: start
    type: start
    config:
      output:
        user_input: "input.user_input"

  - id: planner
    type: supervisor
    config:
      template: tpl_planner
      provider: qwen
      model: qwen-plus
      max_iterations: 10
      max_retry_per_tool: 3
      parallel: false
      timeout: 120000
      tools:
        - name: web_search
          description: "Search the web for up-to-date information on a topic"
          ref: sg_search
        - name: summarize
          description: "Summarize a long text into key points"
          ref: sg_summarize

  - id: format_response
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_format
      variables:
        user_query: "start.user_input"
        task_result: "planner.task_result"

  - id: response
    type: response
    config:
      output:
        answer: "format_response.output"
        execution_report: "planner.task_result"

edges:
  - from: start
    to: planner
  - from: planner
    to: format_response
    exit_reason: default
  - from: format_response
    to: response

subgraphs:
  sg_search:
    nodes:
      - id: search_gen
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: tpl_search_tool
          variables: {}
    edges: []

  sg_summarize:
    nodes:
      - id: summarize_gen
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: tpl_summarize_tool
          variables: {}
    edges: []

templates:
  tpl_planner:
    system_prompt: |
      You are an expert task planner. Given user input, context, and available tools,
      create an execution plan.

      Rules:
      - Each action must have a unique action_id (use descriptive names)
      - action_called must exactly match one of the available tool names
      - action_prerequest lists dependencies by action_id (empty string "" if none)
      - action_detail contains parameters the tool needs (as JSON object)
    user_prompt: |
      ## User Request
      {{user_input}}

      ## Available Context
      {{context}}

      ## Available Tools
      {{tools}}

      Create an execution plan. Output only the JSON array of actions.
    config:
      temperature: 0.3
      max_tokens: 2048

  tpl_search_tool:
    user_prompt: "Search the web for: {{instruction}}"
    output_schema:
      - name: search_results
        type: string

  tpl_summarize_tool:
    user_prompt: "Summarize this text concisely: {{text}}"
    output_schema:
      - name: summary
        type: string

  tpl_format:
    user_prompt: |
      Original question: {{user_query}}

      Task execution results: {{task_result}}

      Based on the results above, provide a clear and helpful response to the user.
    config:
      temperature: 0.5
      max_tokens: 2048
```

The Planner template receives three auto-injected variables: `{{user_input}}`, `{{context}}` (results from prior iterations or "No previous results."), and `{{tools}}` (generated tool descriptions).

---

### workflow -- Sub-Workflow

Executes another workflow as a sub-workflow with isolated state.

**Purpose:** Modular composition -- reuse workflows as building blocks.

#### Configuration

| Field | Description |
|---|---|
| `workflow` | Name of the sub-workflow to execute (must be loaded in the engine) |
| `variables` | Input mapping: `param_name: "parent_state.path"` |
| `output` | Output mapping: `parent_field: "sub_workflow_output_field"` |

#### Referencing another workflow

```yaml
- id: compress
  type: workflow
  config:
    workflow: compress_history
    variables:
      user_id: "start.user_id"
      messages: "start.messages"
    output:
      was_compressed: was_compressed
      compressed_count: compressed_count
```

The sub-workflow `compress_history` must be a separate YAML workflow loaded by the engine. It runs with its own isolated execution state -- only the mapped `variables` are passed as input.

#### Input/output mapping

**Input:** Each entry in `variables` maps a sub-workflow input parameter to a path in the parent state.

**Output:** Each entry in `output` maps a field name in the parent state (left) to a field from the sub-workflow's response node output (right).

#### State isolation

The sub-workflow gets a fresh `ExecutionState`. It cannot read from or write to the parent's state. Only the explicitly mapped variables go in, and only the explicitly mapped outputs come back.

Additional metadata outputs:
- `_status`: Execution status (e.g., `"Completed"`)
- `_timing_ms`: Total execution time in milliseconds
- `_error`: Error message if the sub-workflow failed

#### Working example

```yaml
version: "1.0"
name: "main_workflow"

nodes:
  - id: start
    type: start

  - id: enrich
    type: workflow
    config:
      workflow: enrich_profile
      variables:
        user_id: "start.user_id"
      output:
        profile: profile_data
        score: risk_score

  - id: decide
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: tpl_decide
      variables:
        profile: "enrich.profile"
        score: "enrich.score"

  - id: response
    type: response
    config:
      output:
        decision: "decide.output"
        profile: "enrich.profile"

edges:
  - from: start
    to: enrich
  - from: enrich
    to: decide
  - from: decide
    to: response

templates:
  tpl_decide:
    user_prompt: |
      User profile: {{profile}}
      Risk score: {{score}}

      Based on this profile, make a recommendation.
    config:
      temperature: 0.5
      max_tokens: 512
```

The `enrich_profile` workflow is a separate YAML file with its own `start`, processing nodes, and `response` node. It receives `user_id` as input and returns `profile_data` and `risk_score` in its response.

---

### skill_set -- Skill Definitions

Holds a collection of skill definitions for progressive disclosure. Generate nodes reference skill_set nodes to give the LLM on-demand access to knowledge bundles.

See the [Skill System](#skill-system) section for full details on progressive loading levels, system prompt composition, and edge cases.

#### Configuration

| Field | Required | Description |
|---|---|---|
| `skills` | Yes | Array of skill definitions |
| `skills[].name` | Yes | Unique skill name |
| `skills[].description` | Yes | Short description for L1 metadata |
| `skills[].content` | No* | Inline skill content |
| `skills[].content_file` | No* | External file path, resolved at execution |
| `skills[].references` | No | File paths for L3 reference loading |

*One of `content` or `content_file` is required per skill.

#### Output fields

| Field | Type | Description |
|---|---|---|
| `skills` | array | Resolved skill objects (content_file replaced with content) |
| `_skill_count` | number | Number of skills defined |

#### Example

```yaml
- id: coding_skills
  type: skill_set
  config:
    skills:
      - name: rust_conventions
        description: "Rust coding conventions and best practices"
        content: |
          ## Rust Conventions
          1. Use snake_case for variables and functions
          2. Use CamelCase for types and traits
      - name: api_design
        description: "REST API design guidelines"
        content_file: "skills/api_design.md"
        references:
          - "docs/openapi_spec.md"
```

Connect to a generate node that uses it:

```yaml
edges:
  - from: start
    to: coding_skills
  - from: coding_skills
    to: helper

nodes:
  - id: helper
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: helper_tpl
      skill_sets: ["coding_skills"]
      variables:
        question: "start.question"
```

---

## Data Connectors

Data connector nodes (`type: data_connector`) connect to external databases, caches, object storage, and APIs. Every connector uses the V2 pattern: a `data_source_id` referencing a connection definition, an `operation` block, and a `variables` map for parameterized binding.

> **Important:** The `operation` block must contain an inner `operation` field specifying the type (e.g., `query`, `select`, `insert`, `get`, `set`). This double-nesting is required — `operation.operation` is the type specifier, and sibling fields (`sql`, `table`, `key_template`, etc.) are type-specific parameters.

```yaml
- id: my_node
  type: data_connector
  config:
    data_source_id: my_source       # References data_sources section or Engine-level source
    operation:
      operation: query              # Connector-specific operation type
      sql: "SELECT * FROM t WHERE id = {userId}"
    variables:
      userId: start.user_id         # Pre-resolved, then injected as {userId}
```

---

### PostgreSQL

**Connection config:**

```yaml
data_sources:
  main_db:
    name: "Main Database"
    kind: postgresql                # Also accepts "postgres"
    config:
      # Option A: Connection URL (recommended)
      url: "${DATABASE_URL}"
      # Option B: Individual fields
      host: "localhost"             # Default: localhost
      port: 5432                    # Default: 5432
      username: "app_user"          # Note: use "username" (not "user")
      password: "${PG_PASSWORD}"
      database: "myapp"
```

**Operations:**

#### query -- Raw SQL (most flexible)

```yaml
- id: search_users
  type: data_connector
  config:
    data_source_id: main_db
    operation:
      operation: query
      sql: "SELECT u.id, u.name, o.total
            FROM users u
            JOIN orders o ON o.user_id = u.id
            WHERE u.status = {status}
              AND o.total::float8 >= {minTotal}
            ORDER BY o.total DESC
            LIMIT 20"
    variables:
      status: start.user_status
      minTotal: start.min_order_total
```

Parameters use `{paramName}` syntax. The engine replaces each `{paramName}` with a positional `$1`, `$2`, ... placeholder and binds the resolved value with its native type (string, integer, float, boolean, JSON, UUID, or string array). This is fully parameterized and SQL-injection-safe.

#### select -- Structured query

```yaml
- id: get_users
  type: data_connector
  config:
    data_source_id: main_db
    operation:
      operation: select
      table: users
      columns: ["id", "name", "email"]     # Optional. Default: *
      where:
        status: "{status}"
        role: "{role}"
      order_by: "created_at DESC"          # Optional
      limit: 50                            # Optional
      offset: 0                            # Optional
    variables:
      status: start.filter_status
      role: start.filter_role
```

The `where` field generates `WHERE "status" = $1 AND "role" = $2` with parameterized binding.

#### insert -- Insert with RETURNING

```yaml
- id: create_user
  type: data_connector
  config:
    data_source_id: main_db
    operation:
      operation: insert
      table: users
      data:
        name: "{userName}"
        email: "{userEmail}"
        metadata: "{userMeta}"             # Objects/arrays become JSONB
      returning: ["id", "created_at"]      # Optional. PostgreSQL RETURNING clause
    variables:
      userName: start.name
      userEmail: start.email
      userMeta:
        from: start.metadata
        default: null
```

#### update -- Update with WHERE and RETURNING

```yaml
- id: update_profile
  type: data_connector
  config:
    data_source_id: main_db
    operation:
      operation: update
      table: users
      set:
        name: "{newName}"
        updated_at: "{timestamp}"
      where:
        id: "{userId}"
      returning: ["id", "name", "updated_at"]
    variables:
      newName: start.new_name
      timestamp: build_ts.timestamp
      userId: start.user_id
```

#### delete -- Delete with required WHERE

```yaml
- id: remove_session
  type: data_connector
  config:
    data_source_id: main_db
    operation:
      operation: delete
      table: sessions
      where:
        user_id: "{userId}"
        expired: "true"                    # Boolean literal
    variables:
      userId: start.user_id
```

DELETE requires a WHERE clause for safety. Attempting a delete without WHERE returns an error.

**DECIMAL/NUMERIC type note:** PostgreSQL DECIMAL/NUMERIC columns require an explicit `::float8` cast in parameterized queries because the engine binds parameters as INT8 or FLOAT8, not NUMERIC natively:

```yaml
sql: "SELECT * FROM products WHERE price::float8 >= {minPrice}"
```

**Output fields:**

| Field | Type | Description |
|-------|------|-------------|
| `rows` | array | Array of row objects |
| `count` | number | Number of rows returned/affected |
| _(first row fields)_ | _(varies)_ | Each column of the first row is flattened to top level |

For `query` and `select`, the first row's fields are also available directly (e.g., `node.name` instead of `node.rows[0].name`).

---

### MySQL / MariaDB

**Connection config:**

```yaml
data_sources:
  my_mysql:
    name: "MySQL Production"
    kind: mysql                     # Also accepts "mariadb"
    config:
      host: "localhost"             # Default: localhost
      port: 3306                    # Default: 3306
      username: "root"
      password: "${MYSQL_PASSWORD}"
      database: "myapp"
```

MySQL does not support a `url` field; use individual connection fields.

**Operations:** MySQL supports the same five operations as PostgreSQL: `query`, `select`, `insert`, `update`, `delete`. The `{paramName}` variable binding syntax is identical.

#### query

```yaml
- id: search
  type: data_connector
  config:
    data_source_id: my_mysql
    operation:
      operation: query
      sql: "SELECT * FROM users WHERE name = {name} AND age >= {minAge}"
    variables:
      name: start.search_name
      minAge: start.min_age
```

The engine replaces `{paramName}` with `?` placeholders for MySQL prepared statements.

#### select

```yaml
- id: list_orders
  type: data_connector
  config:
    data_source_id: my_mysql
    operation:
      operation: select
      table: orders
      columns: ["id", "total", "status"]
      where:
        user_id: "{userId}"
      order_by: "created_at DESC"
      limit: 20
    variables:
      userId: start.user_id
```

Columns and tables are quoted with backticks (`` ` ``) instead of PostgreSQL's double quotes.

#### insert

```yaml
- id: create_order
  type: data_connector
  config:
    data_source_id: my_mysql
    operation:
      operation: insert
      table: orders
      data:
        user_id: "{userId}"
        total: "{amount}"
        status: "pending"
      returning: ["id", "total"]           # Emulated via follow-up SELECT
    variables:
      userId: start.user_id
      amount: calculate.total
```

**Differences from PostgreSQL:**

| Feature | PostgreSQL | MySQL |
|---------|-----------|-------|
| RETURNING clause | Native `RETURNING` | Emulated: follow-up `SELECT ... WHERE id = LAST_INSERT_ID()` |
| Identifier quoting | `"column_name"` | `` `column_name` `` |
| Insert output | `rows` (from RETURNING) | `count`, `last_insert_id` |
| Parameterized placeholders | `$1`, `$2`, ... | `?`, `?`, ... |

#### update

```yaml
- id: update_status
  type: data_connector
  config:
    data_source_id: my_mysql
    operation:
      operation: update
      table: orders
      set:
        status: "{newStatus}"
      where:
        id: "{orderId}"
    variables:
      newStatus: start.new_status
      orderId: start.order_id
```

UPDATE requires a WHERE clause. Returns `count` (affected rows). If `returning` is specified, a follow-up SELECT is executed using the WHERE conditions.

#### delete

```yaml
- id: cleanup
  type: data_connector
  config:
    data_source_id: my_mysql
    operation:
      operation: delete
      table: sessions
      where:
        expired: "true"
```

DELETE requires a WHERE clause. Returns `count` (affected rows).

---

### MongoDB

**Connection config:**

```yaml
data_sources:
  analytics_db:
    name: "Analytics MongoDB"
    kind: mongodb                   # Also accepts "mongo"
    config:
      url: "mongodb://host:27017"   # Required. MongoDB connection string
      database: "analytics"         # Default: "test"
```

MongoDB supports 10 operations. All string values in `filter`, `data`, `update`, and `pipeline` support `{paramName}` references resolved from the node's `variables` map. Resolution is recursive: objects and arrays within these fields are traversed, and any string matching `{varName}` is replaced with the variable's value (type-preserving for exact matches).

#### find -- Query multiple documents

```yaml
- id: active_users
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: find
      collection: "users"
      filter:
        status: "active"
        age: { "$gte": 18 }               # MongoDB query operators
      sort: { created_at: -1 }             # Optional
      projection: { name: 1, email: 1 }   # Optional. Field inclusion/exclusion
      limit: 100                           # Optional
      skip: 0                              # Optional
    variables:
      status: start.filter_status
```

**Output:** `rows` (array), `count` (number), plus first document fields flattened to top level.

#### find_one -- Single document lookup

```yaml
- id: get_user
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: find_one
      collection: "users"
      filter:
        _id: "{userId}"
    variables:
      userId: start.user_id
```

**Output:** `document` (object or null), `exists` (boolean), plus document fields flattened if found.

#### insert_one

```yaml
- id: log_event
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: insert_one
      collection: "events"
      data:
        type: "page_view"
        user_id: "{userId}"
        url: "{pageUrl}"
        timestamp: "{ts}"
    variables:
      userId: start.user_id
      pageUrl: start.url
      ts: build_ts.timestamp
```

**Output:** `inserted_id`, `count` (always 1).

#### insert_many

```yaml
- id: bulk_insert
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: insert_many
      collection: "products"
      data:
        - { name: "Widget A", price: 9.99, category: "tools" }
        - { name: "Widget B", price: 19.99, category: "tools" }
```

**Output:** `inserted_ids` (array), `count` (number inserted).

#### update_one

```yaml
- id: upgrade_user
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: update_one
      collection: "users"
      filter:
        _id: "{userId}"
      update:
        status: "premium"
        tier: 2
    variables:
      userId: start.user_id
```

If the `update` document does not contain any `$` operators (like `$set`, `$inc`, `$push`), the engine automatically wraps it in `$set`. To use raw update operators, include them explicitly:

```yaml
      update:
        "$set": { status: "premium" }
        "$inc": { login_count: 1 }
```

**Output:** `matched_count`, `modified_count`, `count` (same as modified_count).

#### update_many

Same syntax as `update_one` but affects all matching documents.

#### delete_one

```yaml
- id: remove_doc
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: delete_one
      collection: "temp_data"
      filter:
        source: "test"
```

**Output:** `deleted_count`, `count`.

#### delete_many

Same syntax as `delete_one` but deletes all matching documents.

#### aggregate -- Pipeline

```yaml
- id: sales_report
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: aggregate
      collection: "orders"
      pipeline:
        - { "$match": { status: "completed", region: "{region}" } }
        - { "$group": { _id: "$category", total: { "$sum": "$amount" }, count: { "$sum": 1 } } }
        - { "$sort": { total: -1 } }
        - { "$limit": 10 }
    variables:
      region: start.region
```

**Output:** `rows` (array of pipeline results), `count`, plus first result fields flattened.

#### count

```yaml
- id: count_active
  type: data_connector
  config:
    data_source_id: analytics_db
    operation:
      operation: count
      collection: "users"
      filter:
        status: "active"
```

**Output:** `count` (number).

---

### Redis

**Connection config:**

```yaml
data_sources:
  cache:
    name: "Redis Cache"
    kind: redis
    config:
      # Option A: Connection URL
      url: "redis://localhost:6379/0"
      # Option B: Individual fields
      host: "localhost"             # Default: localhost
      port: 6379                    # Default: 6379
      password: "${REDIS_PASSWORD}" # Optional
      db: 0                        # Default: 0
```

For production, register connection pools at Engine build time for connection reuse:

```rust
Engine::builder()
    .redis("cache", "redis://localhost:6379/0")
    .build()
    .await?;
```

Redis supports 7 operations. Keys are specified via `key_template` with `{paramName}` interpolation. Values use either `value_template` (flat string) or `value` (structured, type-preserving).

#### get -- Read a key

```yaml
- id: load_session
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: get
      key_template: "session:{userId}"
      output_schema:                       # Optional. Extract typed fields from JSON value
        - name: chat_history
          type: array
        - name: preferences
          type: object
        - name: last_seen
          type: string
    variables:
      userId: start.user_id
```

**Output:** `value` (parsed JSON or raw string), `exists` (boolean), plus any `output_schema` fields. When the key does not exist, `output_schema` fields receive typed empty defaults: `[]` for array, `""` for string, `0` for number, `false` for boolean, `{}` for object.

#### set -- Write a key

```yaml
# Simple string value
- id: cache_result
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: set
      key_template: "result:{queryId}"
      value_template: "{responseText}"
      ttl: 3600                            # Optional. Seconds
    variables:
      queryId: start.query_id
      responseText: generate.text

# Structured value (type-preserving)
- id: save_session
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: set
      key_template: "session:{userId}"
      value:                               # Structured: recursive {var} resolution
        preferences: "{userPrefs}"         # Exact match: injects raw object/array
        last_query: "{queryText}"          # String value
        updated_at: "{timestamp}"
      ttl: 86400
    variables:
      userId: start.user_id
      userPrefs: start.preferences         # Object from input -> injected as object
      queryText: start.query
      timestamp: build_ts.timestamp
```

When a `value` leaf is exactly `"{varName}"` and the variable holds a non-string type (array, object, number, boolean), the raw value is injected directly without stringification. Mixed templates like `"Hello {name}!"` always produce strings. If both `value` and `value_template` are present, `value` takes priority.

**Output:** `ok` (boolean).

#### del -- Delete a key

```yaml
- id: clear_cache
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: del
      key_template: "session:{userId}"
    variables:
      userId: start.user_id
```

**Output:** `ok` (boolean), `deleted` (number: 0 or 1).

#### publish -- Publish to a channel

```yaml
- id: notify
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: publish
      key_template: "events:{channelId}"
      value_template: "{eventPayload}"
    variables:
      channelId: start.channel
      eventPayload: build_event.json_string
```

**Output:** `ok` (boolean), `subscribers` (number of receivers).

#### json_append -- Atomic array append

Atomically appends elements to a JSON array stored at a key, using a Lua script for zero race conditions.

```yaml
- id: append_messages
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: json_append
      key_template: "chat:{userId}:{characterId}"
      path: "chat_history"                 # JSON field name ("" = root must be array)
      value:                               # Structured value
        - role: "user"
          content: "{userMessage}"
        - role: "assistant"
          content: "{assistantMessage}"
      ttl: 86400                           # Optional
    variables:
      userId: start.user_id
      characterId: start.character_id
      userMessage: start.message
      assistantMessage: generate.text
```

When `value` is an array, elements are spread into the target array. A single value is pushed as one element. Use `path: ""` when the root value itself is the array.

**Output:** `ok` (boolean), `length` (new array length).

#### json_merge -- Atomic object merge

Atomically shallow-merges an object into a JSON value at a key.

```yaml
- id: update_meta
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: json_merge
      key_template: "profile:{userId}"
      path: ""                             # "" = merge into root object
      value:
        last_seen: "{timestamp}"
        message_count: "{count}"
      ttl: 86400
    variables:
      userId: start.user_id
      timestamp: build_ts.timestamp
      count: counter.total
```

**Output:** `ok` (boolean).

#### json_delete -- Atomic field deletion

Atomically deletes a field from a JSON object stored at a key.

```yaml
- id: remove_field
  type: data_connector
  config:
    data_source_id: cache
    operation:
      operation: json_delete
      key_template: "profile:{userId}"
      path: "temporary_data"               # Required. Field to remove
      ttl: 86400                           # Optional. Refreshes TTL after deletion
    variables:
      userId: start.user_id
```

**Output:** `ok` (boolean), `deleted` (number: 0 or 1).

**Chat history pattern** (complete example):

```yaml
version: "1.0"
name: "chat_with_memory"

data_sources:
  cache:
    name: "Redis"
    kind: redis
    config:
      url: "${REDIS_URL}"

nodes:
  - id: start
    type: start
    config:
      output:
        userId: input.user_id
        message: input.message

  - id: load_history
    type: data_connector
    config:
      data_source_id: cache
      operation:
        operation: get
        key_template: "chat:{userId}"
        output_schema:
          - name: chat_history
            type: array
      variables:
        userId: start.userId

  - id: generate
    type: generate
    config:
      provider: openai
      model: gpt-4o
      template: chat_template
      variables:
        history: load_history.chat_history
        userMessage: start.message

  - id: save_history
    type: data_connector
    config:
      data_source_id: cache
      operation:
        operation: json_append
        key_template: "chat:{userId}"
        path: "chat_history"
        value:
          - role: "user"
            content: "{userMessage}"
          - role: "assistant"
            content: "{reply}"
        ttl: 86400
      variables:
        userId: start.userId
        userMessage: start.message
        reply: generate.text

  - id: response
    type: response
    config:
      output:
        reply: generate.text

edges:
  - from: start
    to: load_history
  - from: load_history
    to: generate
  - from: generate
    to: save_history
  - from: save_history
    to: response

templates:
  chat_template:
    system_prompt: "You are a helpful assistant."
    user_prompt: |
      {{#if history}}
      Previous conversation:
      {{#each history}}
      {{this.role}}: {{this.content}}
      {{/each}}
      {{/if}}

      User: {{userMessage}}
    config:
      temperature: 0.7
      max_tokens: 2048
```

---

### S3 / MinIO

**Connection config:**

```yaml
data_sources:
  storage:
    name: "Object Storage"
    kind: s3                        # Also accepts "minio"
    config:
      endpoint: "http://localhost:9000"          # Required
      public_endpoint: "https://s3.example.com"  # Optional. Used by presigned_url
      access_key: "${S3_ACCESS_KEY}"             # Required (snake_case!)
      secret_key: "${S3_SECRET_KEY}"             # Required (snake_case!)
      region: "us-east-1"                        # Optional. Default: us-east-1
```

Note: The config uses `access_key` and `secret_key` (snake_case), not camelCase.

#### get_object

```yaml
- id: download
  type: data_connector
  config:
    data_source_id: storage
    operation:
      operation: get_object
      bucket: "my-bucket"
      key: "uploads/{fileKey}"
    variables:
      fileKey: start.file_key
```

Text and JSON content is returned as-is. Binary data is base64-encoded. JSON content is automatically parsed.

**Output:** `data` (parsed content or base64 string), `content_type`, `size`, `last_modified`.

#### put_object

```yaml
- id: upload
  type: data_connector
  config:
    data_source_id: storage
    operation:
      operation: put_object
      bucket: "{userId}"                   # Supports {var} templates
      prefix: "uploads"                    # Optional. Default: "uploads"
      content_type: "auto"                 # Optional. "auto" (default) or explicit MIME type
    variables:
      userId: start.user_id
      data: start.file_base64              # Must be base64-encoded
```

The `data` variable must contain base64-encoded binary data. The engine auto-detects MIME types from magic bytes (JPEG, PNG, GIF, WebP, BMP, MP3, WAV, OGG, M4A) when `content_type` is "auto" or omitted. The bucket is automatically created if it does not exist.

**Output:** `url` (full URL to the object), `bucket`, `key` (generated as `{prefix}/{uuid}.{ext}`).

#### list_objects

```yaml
- id: list_files
  type: data_connector
  config:
    data_source_id: storage
    operation:
      operation: list_objects
      bucket: "my-bucket"
      prefix: "uploads/"                   # Optional
      max_keys: 100                        # Optional. Default: 1000
```

**Output:** `objects` (array of `{key, size, last_modified, etag}`), `truncated` (boolean).

#### delete_object

```yaml
- id: remove_file
  type: data_connector
  config:
    data_source_id: storage
    operation:
      operation: delete_object
      bucket: "my-bucket"
      key: "uploads/{fileKey}"
    variables:
      fileKey: start.file_key
```

**Output:** `deleted` (boolean).

#### head_object

```yaml
- id: check_exists
  type: data_connector
  config:
    data_source_id: storage
    operation:
      operation: head_object
      bucket: "my-bucket"
      key: "uploads/{fileKey}"
    variables:
      fileKey: start.file_key
```

**Output:** `exists` (boolean), `content_type`, `size`, `last_modified`.

#### presigned_url

```yaml
- id: share_link
  type: data_connector
  config:
    data_source_id: storage
    operation:
      operation: presigned_url
      bucket: "my-bucket"
      key: "uploads/{fileKey}"
      expires_in: 3600                     # Optional. Seconds. Default: 3600
    variables:
      fileKey: start.file_key
```

Uses `public_endpoint` from the data source config if available, so the presigned URL points to the public-facing domain rather than the internal endpoint.

**Output:** `url`, `expires_in`.

#### copy_object

```yaml
- id: copy_file
  type: data_connector
  config:
    data_source_id: storage
    operation:
      operation: copy_object
      source_bucket: "staging"
      source_key: "temp/{srcKey}"
      bucket: "production"
      key: "final/{destKey}"
    variables:
      srcKey: start.source_key
      destKey: start.dest_key
```

Server-side copy between buckets or keys. No data transfer through the engine.

**Output:** `bucket`, `key`.

---

### Qdrant (Vector DB)

**Connection config:**

```yaml
data_sources:
  vectors:
    name: "Vector Store"
    kind: qdrant
    config:
      url: "http://localhost:6334"         # Required
      api_key: "${QDRANT_API_KEY}"         # Optional
```

Qdrant supports 7 operations. Search and upsert operations can auto-embed text using a configured embedding provider.

**Embedding provider configuration:**

Embedding providers are registered at Engine build time. The `embedding_provider` field in operations selects which provider to use (default: "dashscope").

#### search -- Similarity search

```yaml
# Auto-embedding mode: provide text, engine generates vector
- id: semantic_search
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: search
      collection: "documents"
      text: "{searchQuery}"                # Text to embed automatically
      embedding_provider: dashscope        # Engine-registered provider (default: dashscope)
      dimension: 1024                      # Optional. Embedding dimension
      limit: 5                             # Optional. Default: 10
      score_threshold: 0.7                 # Optional. Minimum similarity score
      with_payload: true                   # Optional. Default: true
      with_vectors: false                  # Optional. Default: false
      filter:                              # Optional. Qdrant filter
        must:
          - key: "category"
            match: { value: "technical" }
        must_not:
          - key: "archived"
            match: { value: true }
    variables:
      searchQuery: start.query

# Direct vector mode: provide pre-computed vector
- id: vector_search
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: search
      collection: "documents"
      vector: [0.1, 0.2, 0.3, ...]        # Pre-computed embedding
      limit: 10
```

**Filter syntax:** Filters use `must` (AND), `should` (OR), and `must_not` (NOT) arrays. Each condition specifies a `key` and a `match` with a `value` (string, integer, or boolean).

**Output:** `results` (array of `{id, score, payload}`), `count`.

#### upsert -- Insert/update points

```yaml
- id: store_vectors
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: upsert
      collection: "documents"
      text_field: "content"                # Optional. Auto-embed this payload field
      embedding_provider: dashscope
      dimension: 1024
      points:
        - id: "doc-001"                    # String (UUID) or number ID
          payload:
            content: "Machine learning fundamentals..."
            category: "technical"
            author: "Alice"
        - id: "doc-002"
          vector: [0.1, 0.2, ...]          # Or provide vector directly
          payload:
            content: "Data engineering best practices..."
            category: "technical"
```

When `text_field` is set, the engine batches all texts from that payload field and generates embeddings in a single API call. Points with an explicit `vector` field skip embedding.

**Output:** `success` (boolean), `count`.

#### batch_upsert -- Embed and store text array

```yaml
- id: index_sentences
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: batch_upsert
      collection: "sentences"
      texts: "{sentenceArray}"             # Array of strings to embed
      text_field_name: "text"              # Payload field for individual text (default: "text")
      embedding_provider: dashscope
      dimension: 1024
      shared_payload:                      # Additional payload for all points
        source_doc: "{docId}"
        category: "parsed"
    variables:
      sentenceArray: splitter.sentences
      docId: start.document_id
```

Each text gets a UUID point ID. All texts are embedded in a single batch API call.

**Output:** `success` (boolean), `count`, `point_ids` (array of generated UUIDs).

#### scroll -- Paginated listing

```yaml
- id: list_points
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: scroll
      collection: "documents"
      limit: 100                           # Optional. Default: 100
      offset: ""                           # Optional. Point ID for pagination
      with_payload: true
      filter:
        must:
          - key: "category"
            match: { value: "technical" }
```

**Output:** `points` (array of `{id, payload}`), `next_offset` (for pagination), `count`.

#### get_points -- Fetch by IDs

```yaml
- id: fetch_specific
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: get_points
      collection: "documents"
      ids: ["doc-001", "doc-002", "doc-003"]
      with_payload: true
      with_vectors: false
```

**Output:** `points` (array of `{id, payload}`), `count`.

#### delete -- Delete points

```yaml
# Delete by IDs
- id: remove_points
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: delete
      collection: "documents"
      ids: ["doc-001", "doc-002"]

# Delete by filter
- id: remove_by_filter
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: delete
      collection: "documents"
      filter:
        must:
          - key: "archived"
            match: { value: true }
```

**Output:** `success` (boolean).

#### create_collection

```yaml
- id: init_collection
  type: data_connector
  config:
    data_source_id: vectors
    operation:
      operation: create_collection
      collection: "new_documents"
      dimension: 1024                      # Required. Vector dimension
      distance: "Cosine"                   # Optional. Cosine | Euclid | Dot | Manhattan
```

**Output:** `success` (boolean), `collection` (name).

---

### REST API

**Connection config:**

```yaml
data_sources:
  external_api:
    name: "External Service"
    kind: rest_api
    config:
      base_url: "https://api.example.com"
      headers:                             # Optional. Default headers for all requests
        Accept: "application/json"
      auth_type: bearer                    # Optional. bearer | api_key | basic
      auth:
        token: "${API_TOKEN}"              # For bearer auth
```

**Auth modes:**

```yaml
# Bearer token
auth_type: bearer
auth:
  token: "${API_TOKEN}"
# Adds: Authorization: Bearer <token>

# API key
auth_type: api_key
auth:
  api_key: "${API_KEY}"
  api_key_header: "X-API-Key"             # Optional. Default: X-API-Key

# Basic auth
auth_type: basic
auth:
  username: "user"
  password: "${API_PASSWORD}"
```

**V2 operation format:**

```yaml
- id: call_api
  type: data_connector
  config:
    data_source_id: external_api
    operation:
      method: POST                         # GET | POST | PUT | DELETE | PATCH
      path: "/api/v1/users/{userId}"       # Appended to base_url
      headers:                             # Optional. Override/extend data source headers
        X-Request-Id: "{{start.request_id}}"
      query_params:                        # Optional
        page: "1"
        limit: "20"
      body:                                # Optional. Request body
        name: "{userName}"
        email: "{userEmail}"
      body_type: json                      # Optional. json (default) | form | raw
    variables:
      userId: start.user_id
      userName: start.name
      userEmail: start.email
```

**Output:** `status` (HTTP status code), `data` (parsed JSON body), `headers` (response headers).

---

### GraphQL

**Connection config:**

```yaml
data_sources:
  graphql_api:
    name: "GraphQL Service"
    kind: graphql
    config:
      endpoint: "https://api.example.com/graphql"
      headers:
        Authorization: "Bearer ${GQL_TOKEN}"
      auth_type: bearer                    # Optional. Same auth modes as REST
      auth:
        token: "${GQL_TOKEN}"
```

**Query with variables:**

```yaml
- id: fetch_user
  type: data_connector
  config:
    data_source_id: graphql_api
    operation:
      query: |
        query GetUser($id: ID!) {
          user(id: $id) {
            name
            email
            posts {
              title
              publishedAt
            }
          }
        }
      variables:
        id: "{userId}"
    variables:
      userId: start.user_id
```

Note the two `variables` levels: the outer `variables` (under `config`) resolves `{userId}` from state. The inner `variables` (under `operation`) are the GraphQL query variables sent in the request body.

**Output:** `status` (HTTP status code), `data` (the `data` field from the GraphQL response), `errors` (the `errors` field, if present).

---

## Edges & Routing

Edges define the execution graph. Nodes execute as soon as all their incoming edge sources have completed (dependency-based, not layer-based).

```yaml
edges:
  # Unconditional edge
  - from: start
    to: load_data

  # Conditional edge
  - from: analyze
    to: search
    condition: "analyze.needs_search == true"

  # Default edge (taken when no conditional edges from this source match)
  - from: analyze
    to: direct_answer
    is_default: true

  # Loop edge with iteration protection
  - from: refine
    to: check
    max_iterations: 5

  # Supervisor exit edge
  - from: agent
    to: success_handler
    exit_reason: todo_empty

  # Error edge (taken when source node fails with on_exhausted: continue)
  - from: risky_api
    to: fallback_handler
    on_error: true
```

### Edge Types

**Unconditional:** Always taken when the source node completes. The simplest edge.

**Conditional:** Taken only when the `condition` expression evaluates to true. Multiple conditional edges from the same source are evaluated independently (more than one can fire).

**Default:** Taken when no conditional edges from the same source match. Acts as an "else" branch. Only one default edge per source node is meaningful.

**Loop:** An edge where `from` and `to` are the same node (or form a cycle). Must have `max_iterations` set for loop protection. The engine tracks edge traversal counts and stops taking the edge once the limit is reached.

**Exit:** Used exclusively for edges from `supervisor` nodes. The `exit_reason` field determines which exit condition triggers this edge.

**Error:** When `on_error: true`, the edge activates only when the source node has `_error: true` in its outputs (meaning it failed after exhausting retries with `on_exhausted: continue`). Normal edges from the same source are suppressed when on_error edges exist and the source failed.

### on_error Edges (Complete Pattern)

```yaml
nodes:
  - id: primary_api
    type: data_connector
    config:
      data_source_id: external_service
      operation:
        method: GET
        path: "/data"
      error_handling:
        retry: 3
        retry_delay_ms: 2000
        timeout_ms: 10000
        on_exhausted: continue             # Key: don't terminate workflow

  - id: fallback_api
    type: data_connector
    config:
      data_source_id: backup_service
      operation:
        method: GET
        path: "/data"
      variables:
        error_info: "primary_api._error_message"

  - id: merge
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: process_data
      variables:
        data:
          - "primary_api.data"
          - "fallback_api.data"            # Multiple sources: first available wins

edges:
  - from: primary_api
    to: merge                              # Taken when primary succeeds
  - from: primary_api
    to: fallback_api                       # Taken when primary fails
    on_error: true
  - from: fallback_api
    to: merge
```

> **Tip:** When designing error-fallback workflows, use the **multiple sources** variable binding pattern (as shown with `data:` above) to seamlessly read from whichever node succeeded. The response/downstream node uses `["primary.field", "fallback.field"]` so it picks up whichever path produced output.

### Supervisor Exit Edges

```yaml
edges:
  - from: agent
    to: success_handler
    exit_reason: todo_empty                # All tasks completed

  - from: agent
    to: timeout_handler
    exit_reason: timeout                   # Execution time exceeded

  - from: agent
    to: iteration_handler
    exit_reason: max_iterations            # Loop limit reached

  - from: agent
    to: length_handler
    exit_reason: max_result_length         # Result too long

  - from: agent
    to: failure_handler
    exit_reason: tool_failures             # Too many tool errors

  - from: agent
    to: default_handler
    exit_reason: default                   # Catches any exit reason
```

### Condition Expression Language

Conditions are evaluated against the execution state. The expression language supports:

**Comparison operators:**

```
node.field == "value"          # Equality (strings, numbers, booleans)
node.field != 0                # Inequality
node.field > 10                # Greater than (numeric only)
node.field >= 5                # Greater than or equal
node.field < 100               # Less than
node.field <= 50               # Less than or equal
```

**Logical operators:**

```
condition1 && condition2       # AND
condition1 || condition2       # OR
!condition                     # NOT
(a == 1 || b == 2) && c == 3  # Grouping with parentheses
```

**Built-in functions:**

```
empty(node.field)              # true if null, empty string, empty array, or empty object
exists(node.field)             # true if not null
len(node.field)                # returns length (string length, array length, object key count)
```

**Truthiness:** A bare path (without comparison) is truthy if: not null, not false, not 0, not empty string, not empty array, not empty object.

**Cross-type equality:** `"true"` == `true` and `"false"` == `false` for string-boolean comparisons.

### Dead Path Detection

When a conditional edge evaluates to false, the engine propagates dead path information through the graph. A node is marked dead when all its incoming edges are inactive (either from dead source nodes or from false conditions). Dead path detection propagates transitively.

Impact:
- Dead nodes are removed from the pending set and never execute
- Variables with `default` that reference dead nodes use the default value immediately
- Variables without `default` that reference dead nodes cause a clear error message

---

## Templates

Templates define LLM prompts with Handlebars syntax and optional structured output schemas.

```yaml
templates:
  my_template:
    system_prompt: "You are an expert analyst."         # Optional
    user_prompt: |                                      # Required
      {{#if context}}
      Context:
      {{context}}
      {{/if}}

      {{#each history}}
      {{this.role}}: {{this.content}}
      {{/each}}

      Question: {{user_message}}
    config:                                             # Optional
      temperature: 0.7
      max_tokens: 2048
      top_p: 0.95
      top_k: 40
      thinking_budget: 10000
      reasoning_effort: "high"
      enable_search: false
    output_schema:                                      # Optional
      - name: answer
        type: string
        description: "The analytical answer"
      - name: confidence
        type: number
        description: "Confidence score 0-1"
        example: "0.85"
```

### Handlebars Syntax

| Syntax | Description |
|--------|-------------|
| `{{variable}}` | Insert variable value |
| `{{#if var}}...{{/if}}` | Conditional block (renders if var is truthy) |
| `{{#if var}}...{{else}}...{{/if}}` | If-else |
| `{{#each array}}{{this}}{{/each}}` | Iterate over array |
| `{{#each array}}{{this.field}}{{/each}}` | Iterate and access fields |
| `{{#unless var}}...{{/unless}}` | Render if var is falsy |

Variables referenced in templates must be defined in the generate node's `variables` map.

> **Object/array variables:** Non-primitive variable values (objects, arrays) are automatically serialized to JSON strings before template rendering, so `{{my_object}}` will render as a JSON string like `{"key":"value"}`. To access individual fields of an object, use `{{my_object.field}}` dot notation or pre-process with a transform node.

### Config Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `temperature` | float | 0.7 | Sampling temperature (clamped to provider/model max) |
| `max_tokens` | int | 2048 | Maximum tokens to generate |
| `top_p` | float | null | Top-p nucleus sampling (masked if unsupported) |
| `top_k` | int | null | Top-k sampling (masked if unsupported) |
| `thinking_budget` | int | null | Extended thinking token budget (Anthropic, DeepSeek) |
| `reasoning_effort` | string | null | OpenAI o-series reasoning: "low", "medium", "high" |
| `enable_search` | bool | false | Enable web search for supported providers (Qwen/DashScope) |

All parameters are subject to capability masking -- the engine automatically removes parameters that the selected provider/model does not support.

### Output Schema

Output schema forces the LLM to respond in structured JSON. Supported types:

`string`, `number`, `boolean`, `string[]`, `number[]`, `object`, `object[]`

**Flat schema:**

```yaml
output_schema:
  - name: category
    type: string
    description: "The classification category"
  - name: score
    type: number
    example: "0.95"
  - name: tags
    type: string[]
```

**Nested schema with children:**

```yaml
output_schema:
  - name: todos
    type: object[]
    children:
      - name: action_id
        type: string
        example: "search_web"
      - name: action_called
        type: string
        description: "Tool name to invoke"
      - name: action_detail
        type: object
        children:
          - name: query
            type: string
          - name: limit
            type: number
```

When `output_schema` is present, the engine:
1. Sends `response_format: json_object` to providers that support it
2. Appends an example JSON structure to the user prompt
3. Parses the response with robust extraction (handles markdown fences, prose wrapping, brace matching)
4. Maps schema fields to node outputs with type-appropriate defaults for missing fields

> **Reliability note:** Output schema works best with providers that have native JSON mode (`openai`, `gemini`, `qwen`, `anthropic`). Providers without native JSON mode (e.g., `huoshan` doubao models) rely on prompt-based JSON instruction + regex extraction, which may fail if the LLM returns pure prose. When JSON parsing fails, schema fields receive typed defaults (`""` for string, `0` for number, `false` for boolean, `[]` for arrays) and the raw LLM text is available as `gen.output`. Always test structured output with your specific provider/model combination. For multi-step workflows that depend on schema fields, consider adding fallback logic via transform nodes.

---

## Providers Declaration

Providers can be declared at the workflow YAML level, allowing self-contained workflows with their own API keys.

```yaml
providers:
  openai:
    api_key: "${OPENAI_API_KEY}"           # Supports ${ENV_VAR} expansion

  anthropic:
    api_key: "${ANTHROPIC_API_KEY}"

  my_local_llm:
    kind: openai_compatible                # Required for custom endpoints
    api_key: "not-needed"                  # Still required (can be any string)
    base_url: "http://localhost:8000/v1/chat/completions"
    default_model: "llama-3-8b"            # Model to use when not specified in node

  azure_openai:
    kind: openai_compatible
    api_key: "${AZURE_API_KEY}"
    base_url: "https://my-resource.openai.azure.com/openai/deployments/gpt-4o/chat/completions"
    default_model: "gpt-4o"
```

**Fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `api_key` | Yes | API key. Supports `${ENV_VAR}` expansion at parse time |
| `kind` | No | Provider type. Defaults to the provider ID (map key). Use `openai_compatible` for custom endpoints |
| `base_url` | No | Custom endpoint URL. For proxies, Azure OpenAI, vLLM, Ollama, etc. |
| `default_model` | No | Default model override. Primarily for `openai_compatible` |

**Built-in provider kinds:** `openai`, `anthropic`, `gemini`, `deepseek`, `xai`, `qwen`, `zhipu`, `moonshot`, `minimax`, `huoshan`, `openai_compatible`.

> **DashScope routing:** `zhipu` and `moonshot` providers are routed through **DashScope** (Alibaba Cloud). Use a DashScope/Qwen API key (same as `qwen`), not native Zhipu or Moonshot API keys. To use native endpoints, declare an `openai_compatible` provider with the native `base_url`.

> **Thinking models and max_tokens:** Models with thinking/reasoning capabilities (e.g., `doubao-seed-*`, `deepseek-reasoner`, `o4-mini`) may produce tokens beyond the `max_tokens` limit because thinking tokens are counted separately. This is expected behavior.

**Priority:** Workflow-level providers override Engine-level providers with the same ID.

**BYOK vs hosted mode:** When the Engine is built without any providers and the workflow declares its own, the workflow is fully self-contained (BYOK -- Bring Your Own Key). When both Engine-level and workflow-level providers exist, workflow providers take precedence.

---

## Harness Instructions

Harness provides **workflow-level instructions** that are injected into every `generate` node's `system_prompt`, before the template's own system prompt. Think of it as the project's "CLAUDE.md" — always-present rules that every LLM call must follow.

```yaml
harness:
  instructions: |
    你是专业的数据分析助手。
    规则：
    1. 始终用中文回复
    2. 翻译要自然流畅
  files:
    - "rules/output_format.md"
    - "rules/safety_rules.md"
  memory_file: "harness_memory.json"
```

**Fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `instructions` | No | Inline instruction text |
| `files` | No | List of external file paths to read and inject |
| `memory_file` | No | Path to a persistent JSON memory file. Read at start, updated after execution. |

**How it works:**
1. At workflow start, the engine reads `instructions` + all `files` + `memory_file` content
2. Concatenates them with `---` separators
3. For every `generate` node, prepends this text before the template's `system_prompt`

**System prompt composition order:**
```
[harness instructions]    ← highest priority, always first
---
[skill metadata]          ← from skill_set nodes (see Skill System section)
---
[template system_prompt]  ← from the template
```

**Use cases:**
- Project-wide coding standards
- Output format requirements
- Safety rules and guardrails
- Persistent agent memory (via `memory_file` + `writeback`)

**Backward compatibility:** Workflows without `harness:` work identically to before.

### Post-Execution Writeback

When `harness.writeback` is configured, the engine performs a dedicated LLM call after workflow execution to review results and update the memory file. This enables the harness to **learn user habits** across runs.

```yaml
harness:
  instructions: |
    You are a helpful assistant. Always be concise.
  memory_file: "harness_memory.txt"
  writeback:
    provider: deepseek
    model: deepseek-chat
    memory_char_limit: 2200
    prompt: |
      Review the execution results and update memory with
      key user preferences and important context.
```

**WritebackConfig fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `provider` | Yes | LLM provider for the writeback call |
| `model` | Yes | Model to use (cheap/fast model recommended) |
| `memory_char_limit` | No | Max chars for memory file. Default: 2200 (~800 tokens) |
| `prompt` | No | Custom writeback prompt. Default focuses on user preferences, habits, key decisions |

**Behavior:**
- Fires after successful workflow execution
- **Best-effort**: failure does NOT fail the workflow (logged as warning)
- Current run uses a **frozen snapshot** — writeback changes take effect on the next run
- Memory is truncated to `memory_char_limit` to prevent unbounded growth
- The writeback LLM receives: current memory content + workflow final output
- Requires `memory_file` to be set

**Design philosophy:** Harness has two roles — constraining LLM behavior (`instructions`) and learning user habits (`writeback`). Skills, tools, and context expand what the LLM *can* do; harness is the only layer that restricts and remembers.

---

## Skill System

Skills are **progressive disclosure knowledge bundles** — reusable expertise that an LLM loads on demand rather than receiving all at once. This keeps system prompts lean while making deep knowledge available when the LLM needs it.

Skills are defined as `skill_set` graph nodes (visible in the IDE, composable) rather than workflow-level data.

### Three Loading Levels

```
Level 1: Metadata (always in system_prompt)
  ┌──────────────────────────────────────────┐
  │ ## Available Skills                       │
  │ - rust_conventions: Rust coding standards │
  │ - api_design: REST API design guidelines  │
  │ (name + description, ~50-100 tokens each) │
  └──────────────────────────────────────────┘

Level 2: Full content (LLM calls load_skill)
  LLM: "I need Rust conventions for this review"
  → tool_call: load_skill(name: "rust_conventions")
  → returns full skill content as tool result
  → LLM uses knowledge in response

Level 3: References (LLM calls load_skill_reference)
  Skill content references: "docs/openapi_spec.md"
  → tool_call: load_skill_reference(skill_name: "api_design", path: "docs/openapi_spec.md")
  → returns referenced file content
  → only allowed for paths declared in skill's references array
```

### skill_set Node Configuration

Define skills in a `skill_set` node:

```yaml
nodes:
  - id: coding_skills
    type: skill_set
    config:
      skills:
        - name: rust_conventions
          description: "Rust coding conventions and best practices"
          content: |
            ## Rust Conventions
            1. Use snake_case for variables and functions
            2. Use CamelCase for types and traits
            3. Prefer Result<T, E> over panicking
        - name: api_design
          description: "REST API design guidelines"
          content_file: "skills/api_design.md"
          references:
            - "docs/openapi_spec.md"
```

**Skill fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier used in `load_skill` calls |
| `description` | Yes | Short text shown in L1 metadata |
| `content` | No* | Inline skill content (the L2 full text) |
| `content_file` | No* | External file path, resolved to `content` at execution |
| `references` | No | File paths available for L3 loading |

*One of `content` or `content_file` is required.

At execution, the `skill_set` node resolves any `content_file` into `content` and writes the resolved skills array to state. Outputs: `skills` (array), `_skill_count` (number).

### Generate Node Integration

A `generate` node references skill sets via the `skill_sets` config field:

```yaml
- id: helper
  type: generate
  config:
    provider: deepseek
    model: deepseek-chat
    template: helper_tpl
    skill_sets: ["coding_skills"]
    variables:
      question: "start.question"
```

The `skill_set` node must be connected via edges to the `generate` node that references it.

When `skill_sets` is configured, the engine:
1. Reads resolved skills from each referenced skill_set node's state
2. Builds an `## Available Skills` metadata section listing name + description
3. Injects this metadata into system_prompt (middle layer between harness and template)
4. Registers `load_skill` and `load_skill_reference` as built-in tools alongside any user-defined tools
5. During the tool calling loop, dispatches calls to these built-in tools directly (no subgraph execution)

### System Prompt Composition

With skills, the full system prompt composition is:

```
[harness instructions]           ← workflow-level rules (highest priority)
---
[## Available Skills             ← skill metadata from skill_set nodes
 - skill_name: description
 ...]
---
[template system_prompt]         ← from the template
```

### Complete Example

```yaml
version: "1.0"
name: "skill_assisted_helper"

nodes:
  - id: start
    type: start

  - id: coding_skills
    type: skill_set
    config:
      skills:
        - name: rust_conventions
          description: "Rust coding conventions and best practices"
          content: |
            ## Rust Conventions
            1. Use snake_case for variables and functions
            2. Use CamelCase for types and traits
            3. Prefer Result<T, E> over panicking
        - name: api_design
          description: "REST API design guidelines"
          content_file: "skills/api_design.md"
          references:
            - "docs/openapi_spec.md"

  - id: helper
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: helper_tpl
      skill_sets: ["coding_skills"]
      variables:
        question: "start.question"

  - id: response
    type: response
    config:
      output:
        answer: "helper.output"

edges:
  - from: start
    to: coding_skills
  - from: coding_skills
    to: helper
  - from: helper
    to: response

templates:
  helper_tpl:
    user_prompt: "{{question}}"
    system_prompt: "You are a helpful coding assistant."
    config:
      temperature: 0.7
      max_tokens: 2048
```

### Edge Cases

- **Naming conflicts:** User-defined tools cannot be named `load_skill` or `load_skill_reference`. The engine rejects these at build time.
- **Model without function calling:** If the model does not support function calling (`functionCalling: false`), skill metadata is still injected into the system prompt (L1 works), but `load_skill` / `load_skill_reference` tools are not registered (L2/L3 unavailable). The LLM must work with metadata alone.
- **Duplicate skill names:** Within a single `skill_set` node, skill names must be unique. Across multiple `skill_set` nodes referenced by the same generate node, names must also be unique — the engine merges all referenced skill sets into a single index.

---

## MCP Integration

MCP (Model Context Protocol) enables workflows to call tools hosted on external MCP servers. rp-engine supports two scenarios:

### Scenario A: Explicit MCP Tool Call (`mcp` node)

Use when you know exactly which tool to call and with what arguments. The `mcp` node calls the tool directly and writes the result to state.

```yaml
mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"

nodes:
  - id: wiki
    type: mcp
    config:
      server: deepwiki
      tool: read_wiki_structure
      variables:
        repoName: "anthropics/anthropic-cookbook"
```

Output: `wiki.result` contains the tool's return value.

### Scenario B: LLM-Driven MCP Tool Calling (`mcp_tools` + generate)

Use when you want the LLM to discover and call MCP tools autonomously. This follows the same pattern as `skill_sets`:

1. An `mcp_tools` node connects to the server and discovers available tools
2. A `generate` node references it via `mcp_tool_sets`
3. The LLM sees tool definitions and calls them as needed during the tool calling loop

```yaml
mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"

nodes:
  - id: start
    type: start

  - id: wiki_tools
    type: mcp_tools
    config:
      server: deepwiki

  - id: agent
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: agent_tpl
      mcp_tool_sets: ["wiki_tools"]
      variables:
        question: "start.question"

  - id: response
    type: response
    config:
      output:
        answer: "agent.output"

edges:
  - from: start
    to: wiki_tools
  - from: wiki_tools
    to: agent
  - from: agent
    to: response

templates:
  agent_tpl:
    user_prompt: "{{question}}"
    system_prompt: "You are a helpful assistant with wiki access."
    config:
      temperature: 0.7
      max_tokens: 4096
```

MCP tools are injected into the generate node's tool_defs alongside user subgraph tools and skill tools. The LLM autonomously decides which tools to call.

### MCP Server Authentication

```yaml
mcp_servers:
  public_server:
    url: "https://mcp.example.com/mcp"    # No auth needed

  bearer_auth:
    url: "https://private.example.com/mcp"
    auth:
      type: bearer
      token: "${MCP_TOKEN}"               # Authorization: Bearer <token>

  api_key_auth:
    url: "https://api.example.com/mcp"
    auth:
      type: api_key
      token: "${API_KEY}"                 # X-API-Key: <token>
```

MCP servers can also be registered at Engine build time via `.mcp_server(id, url)` or `.mcp_server_def(id, def)`. YAML-level definitions override Engine-level ones with the same ID.

### Protocol Details

- Transport: Streamable HTTP (POST to server URL)
- Wire format: JSON-RPC 2.0
- Response handling: supports both plain JSON and SSE (`text/event-stream`)
- Session management: `Mcp-Session-Id` header

---

## Variable Binding

Variables connect nodes by mapping upstream outputs to downstream inputs. There are three forms plus literal values.

### Form 1: Simple reference

```yaml
variables:
  userName: "start.user_name"              # Blocks until start node completes
```

### Form 2: With default value

```yaml
variables:
  imageUrl:
    from: "gen_image.url"
    default: ""                            # Used when gen_image is on a dead path
```

The `default` value fires only when the source node is confirmed unreachable (dead path), not immediately. This prevents premature execution while avoiding infinite hangs on conditional branches.

### Form 3: Multiple sources (first available)

```yaml
variables:
  # Array form: first available wins
  data:
    - "primary_api.data"
    - "fallback_api.data"

  # Object form with default
  data:
    from:
      - "primary_api.data"
      - "fallback_api.data"
    default: null
```

### Path Expressions

```
node_id.field                  # Latest execution output
node_id[-1].field              # Same as above (explicit last)
node_id[0].field               # First execution output
node_id[*].field               # All executions of that field (returns array)
input.field                    # Workflow input
```

The `[*]` syntax is useful for loop nodes that execute multiple times -- it collects the specified field from every execution into an array.

### Literal Values

When a source string contains no `.` (dot), it is treated as a literal value:

```yaml
variables:
  limit: "10"                  # Parsed as Number(10)
  enabled: "true"              # Parsed as Bool(true)
  label: "hello"               # Kept as String("hello")
  pi: "3.14"                   # Parsed as Number(3.14)
```

Literal parsing priority: integer, float, boolean, string.

### Dead Path Awareness

Variables with `default` are dead-path aware. The engine waits until every source node is settled (completed or confirmed unreachable) before falling back to the default. This prevents:
1. **Premature execution:** Default firing before a live source finishes
2. **Infinite hang:** Variable blocking forever on an untaken conditional branch

If a variable references a dead-path node without a default, the engine reports a clear error.

---

## Subgraphs

Subgraphs are mini-workflows that can be executed by supervisor nodes, iterator nodes, and generate nodes (via tool calling).

### Definition Format

```yaml
subgraphs:
  my_subgraph:
    nodes:
      - id: node_1
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: sub_template
          variables: {}                    # Populated from tool arguments at runtime
      - id: node_2
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: process_template
          variables:
            input_data: node_1.result
      - id: result
        type: response
        config:
          output:
            final_result: node_2.text
    edges:
      - from: node_1
        to: node_2
      - from: node_2
        to: result
```

### How They Are Used

**By supervisor nodes:** The supervisor's LLM generates a todo list with tool names. For each todo item, the engine finds the matching tool's `ref` (subgraph ID), creates an isolated ExecutionState with the tool arguments as input, and executes the subgraph. The subgraph's response node output becomes the tool result.

**By iterator nodes:** The iterator resolves its `items` binding to an array, then runs the referenced subgraph once per element. Each iteration gets `input.{item_var}` (current element) and `input.{index_var}` (current index) plus any shared variables.

**By generate nodes (tool calling):** When a generate node has `tools` configured and the LLM returns tool_call requests, the engine executes the referenced subgraph for each tool call. The subgraph receives the tool arguments as input, and its response output is sent back to the LLM as the tool result.

### Input/Output Conventions

- **Input:** Subgraphs receive input via the `input` namespace in their isolated ExecutionState. For supervisor tools, tool arguments are available as `input.{arg_name}`. For iterators, the current item is `input.{item_var}`.
- **Output:** Subgraphs must include a `response` node. The response node's `output` mapping defines what is returned to the caller. For supervisor tools, this becomes the tool result. For iterators, this becomes one element in the `results` array.
- **Start node:** Subgraphs do not require a `start` node. If omitted, the first node in the list with all dependencies met will execute first.

---

## Common Patterns

### MCP Agent: LLM-Driven Tool Calling via MCP

```yaml
version: "1.0"
name: "mcp_agent"

mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"

nodes:
  - id: start
    type: start

  - id: wiki_tools
    type: mcp_tools
    config:
      server: deepwiki

  - id: agent
    type: generate
    config:
      provider: deepseek
      model: deepseek-chat
      template: agent_tpl
      mcp_tool_sets: ["wiki_tools"]
      variables:
        question: "start.question"

  - id: response
    type: response
    config:
      output:
        answer: "agent.output"

edges:
  - from: start
    to: wiki_tools
  - from: wiki_tools
    to: agent
  - from: agent
    to: response

templates:
  agent_tpl:
    user_prompt: "{{question}}"
    system_prompt: "You are a helpful assistant. Use wiki tools to research answers."
    config:
      temperature: 0.7
      max_tokens: 4096
```

### Error Fallback: Primary with Backup Provider

```yaml
version: "1.0"
name: "resilient_generation"

nodes:
  - id: start
    type: start

  - id: primary
    type: generate
    config:
      provider: openai
      model: gpt-4o
      template: my_prompt
      variables:
        input: start.user_input
      error_handling:
        retry: 2
        timeout_ms: 15000
        on_exhausted: continue

  - id: fallback
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: my_prompt
      variables:
        input: start.user_input

  - id: response
    type: response
    config:
      output:
        result:
          - primary.text
          - fallback.text

edges:
  - from: start
    to: primary
  - from: primary
    to: response                           # Taken when primary succeeds
  - from: primary
    to: fallback                           # Taken when primary fails
    on_error: true
  - from: fallback
    to: response
```

### Batch Processing: Query then Iterate then Aggregate

```yaml
version: "1.0"
name: "batch_summarize"

nodes:
  - id: start
    type: start

  - id: fetch_docs
    type: data_connector
    config:
      data_source_id: main_db
      operation:
        operation: query
        sql: "SELECT id, content FROM documents WHERE status = 'pending' LIMIT 50"

  - id: process_each
    type: iterator
    config:
      items: fetch_docs.rows
      item_var: doc
      index_var: idx
      subgraph: summarize_one
      parallel: true
      max_concurrency: 5
      on_error: collect

  - id: response
    type: response
    config:
      output:
        summaries: process_each.results
        total: process_each.count
        errors: process_each.errors

edges:
  - from: start
    to: fetch_docs
  - from: fetch_docs
    to: process_each
  - from: process_each
    to: response

subgraphs:
  summarize_one:
    nodes:
      - id: gen
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: summarize_tpl
          variables:
            content: input.doc
            index: input.idx
      - id: result
        type: response
        config:
          output:
            summary: gen.summary
            doc_id: input.doc
    edges:
      - from: gen
        to: result

templates:
  summarize_tpl:
    user_prompt: "Summarize the following document:\n\n{{content}}"
    config:
      temperature: 0.3
      max_tokens: 500
    output_schema:
      - name: summary
        type: string
```

### Cache Pattern: Check Redis, Generate if Miss, Store

```yaml
version: "1.0"
name: "cached_generation"

data_sources:
  cache:
    name: "Redis"
    kind: redis
    config:
      url: "${REDIS_URL}"

nodes:
  - id: start
    type: start
    config:
      output:
        query: input.query

  - id: check_cache
    type: data_connector
    config:
      data_source_id: cache
      operation:
        operation: get
        key_template: "answer:{queryHash}"
        output_schema:
          - name: cached_answer
            type: string
      variables:
        queryHash: start.query

  - id: generate
    type: generate
    config:
      provider: openai
      model: gpt-4o
      template: answer_template
      variables:
        query: start.query

  - id: store_cache
    type: data_connector
    config:
      data_source_id: cache
      operation:
        operation: set
        key_template: "answer:{queryHash}"
        value_template: "{answer}"
        ttl: 3600
      variables:
        queryHash: start.query
        answer: generate.text

  - id: response
    type: response
    config:
      output:
        answer:
          - check_cache.cached_answer
          - generate.text

edges:
  - from: start
    to: check_cache
  - from: check_cache
    to: response
    condition: "check_cache.exists == true"
  - from: check_cache
    to: generate
    condition: "check_cache.exists == false"
  - from: generate
    to: store_cache
  - from: store_cache
    to: response

templates:
  answer_template:
    user_prompt: "Answer the following question:\n\n{{query}}"
    config:
      temperature: 0.7
      max_tokens: 2048
```

### Multi-Model Comparison: Parallel Generates

```yaml
version: "1.0"
name: "model_comparison"

nodes:
  - id: start
    type: start

  - id: gpt4
    type: generate
    config:
      provider: openai
      model: gpt-4o
      template: shared_prompt
      variables:
        question: start.user_input

  - id: claude
    type: generate
    config:
      provider: anthropic
      model: claude-sonnet-4-20250514
      template: shared_prompt
      variables:
        question: start.user_input

  - id: qwen
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: shared_prompt
      variables:
        question: start.user_input

  - id: response
    type: response
    config:
      output:
        gpt4_answer: gpt4.text
        claude_answer: claude.text
        qwen_answer: qwen.text

edges:
  - from: start
    to: gpt4                               # All three run in parallel
  - from: start
    to: claude
  - from: start
    to: qwen
  - from: gpt4
    to: response                           # Response waits for all three
  - from: claude
    to: response
  - from: qwen
    to: response

templates:
  shared_prompt:
    user_prompt: "{{question}}"
    config:
      temperature: 0.7
      max_tokens: 2048
```

### Agent with Tools: Generate + Tool Calling + Subgraphs

```yaml
version: "1.0"
name: "tool_calling_agent"

data_sources:
  main_db:
    name: "Database"
    kind: postgresql
    config:
      url: "${DATABASE_URL}"

nodes:
  - id: start
    type: start

  - id: agent
    type: generate
    config:
      provider: openai
      model: gpt-4o
      template: agent_prompt
      variables:
        user_input: start.user_input
      tools:
        - name: search_database
          description: "Search the database for user information"
          ref: sg_search_db
          parameters:
            type: object
            properties:
              query:
                type: string
                description: "SQL-safe search term"
            required: ["query"]
        - name: calculate
          description: "Evaluate a mathematical expression"
          ref: sg_calculate
          parameters:
            type: object
            properties:
              expression:
                type: string
                description: "Math expression to evaluate"
            required: ["expression"]
      max_tool_rounds: 5

  - id: response
    type: response
    config:
      output:
        answer: agent.text

edges:
  - from: start
    to: agent
  - from: agent
    to: response

subgraphs:
  sg_search_db:
    nodes:
      - id: query
        type: data_connector
        config:
          data_source_id: main_db
          operation:
            operation: query
            sql: "SELECT id, name, email FROM users WHERE name ILIKE {searchTerm} LIMIT 10"
          variables:
            searchTerm: input.query
      - id: result
        type: response
        config:
          output:
            users: query.rows
            count: query.count
    edges:
      - from: query
        to: result

  sg_calculate:
    nodes:
      - id: compute
        type: transform
        config:
          script: |
            let expr = expression;
            let result = eval(expr);
            #{ "result": result }
          variables:
            expression: input.expression
      - id: result
        type: response
        config:
          output:
            answer: compute.result
    edges:
      - from: compute
        to: result

templates:
  agent_prompt:
    system_prompt: |
      You are a helpful assistant with access to a database and calculator.
      Use the search_database tool to look up user information.
      Use the calculate tool for math operations.
      Answer the user's question based on tool results.
    user_prompt: "{{user_input}}"
    config:
      temperature: 0.3
      max_tokens: 2048
```
