# RicePrompt Flow YAML Specification v1.0

> This is the authoritative specification for writing workflow YAML files for rp-engine.
> Both the engine and the IDE must conform to this spec.

## Top-Level Structure

```yaml
version: "1.0"                           # Required. "1.0" or "0.1" (legacy)
name: "workflow_name"                    # Required. Unique identifier
description: "What this workflow does"   # Optional

input:                                   # Optional. Input schema
  fields:
    - name: field_name
      type: string                       # string | number | boolean | object | array
      required: true                     # Default: false
      default: "value"                   # Optional. Used when required=false and not provided

data_sources:                            # Optional. Workflow-level data source definitions
  source_id:                             # These override Engine-level sources with the same ID
    name: "Human Name"
    kind: postgresql                     # postgresql | redis | qdrant | minio | rest_api | graphql
    description: "Optional"
    config:
      url: "${DATABASE_URL}"             # Kind-specific connection config

nodes:                                   # Required. At least start + response
  - id: node_id
    type: node_type
    config: { ... }

edges:                                   # Required
  - from: source_id
    to: target_id

templates:                               # Optional. Inline prompt templates
  template_id:
    user_prompt: "..."
    system_prompt: "..."
    config: { ... }
    output_schema: [...]

subgraphs:                               # Optional. For supervisor nodes
  subgraph_id:
    nodes: [...]
    edges: [...]
```

---

## Variable Binding

All nodes that accept input from other nodes use the `variables` field.
There is exactly **one syntax** for variable binding:

```yaml
variables:
  # Form 1: Simple reference (required — errors if source doesn't exist)
  varName: "node_id.field"

  # Form 2: With default value (optional — uses default if source missing)
  varName:
    from: "node_id.field"
    default: ""

  # Form 3: Multiple sources (first available wins, errors if none exist)
  varName:
    - "node_a.field"
    - "node_b.field"

  # Form 4: Multiple sources with default (optional)
  varName:
    from:
      - "node_a.field"
      - "node_b.field"
    default: null
```

### Path Expressions

Variables reference node outputs using path expressions:

```
node_id.field           # Latest execution output
node_id[-1].field       # Same as above (explicit)
node_id[0].field        # First execution output
node_id[*].field        # All executions (returns array)
input.field             # Workflow input
```

### Template Placeholders

Within SQL, key templates, and value templates, reference pre-resolved variables with `{varName}`:

```yaml
operation:
  sql: "SELECT * FROM users WHERE id = {userId}::uuid AND name = {name}"
variables:
  userId: start.user_id
  name: start.user_name
```

> **DEPRECATED**: The legacy `{{node.field}}` inline syntax is deprecated and will be removed.
> Always use `variables` + `{varName}`.

---

## Node Types

### `start` — Entry Point

Every workflow has exactly one start node. Maps workflow input to named outputs.

```yaml
- id: start
  type: start
  config:
    output:                              # Optional. Remap input fields
      local_name: "input.field_name"
```

**Output**: Each key in `output` becomes a field on the start node (e.g., `start.local_name`).
If `output` is omitted, all input fields pass through as-is.

---

### `generate` — LLM Call

Calls an LLM provider with a rendered prompt template.

```yaml
- id: my_llm
  type: generate
  config:
    provider: qwen                       # Required. Engine-registered provider ID
    model: qwen-plus                     # Required. Model ID
    template: my_template                # Required. Template ID (see Templates section)
    stream: false                        # Optional. Default: false
    variables:                           # Optional. Template variable bindings
      user_message: start.user_content
      history:
        from: load_history.rows
        default: ""
```

**Conditional template selection:**

```yaml
    template:
      default: normal_template
      conditions:
        - when: "check.should_trigger == true"
          use: special_template
```

**Output fields:**
- `{node_id}.text` — Generated text (always present)
- `{node_id}.{schema_field}` — Parsed JSON fields when `output_schema` is defined
- `{node_id}.input_tokens` / `{node_id}.output_tokens` — Token usage
- `{node_id}.actual_prompt` — Rendered prompt (when `capture_actual_prompt` enabled)
- `{node_id}.raw_response` — Raw LLM response (when `capture_raw_response` enabled)

**Streaming**: When `stream: true`, the engine calls `provider.stream()` and forwards chunks
to the registered `StreamHandler`. Streaming is automatically disabled when `output_schema`
is present (JSON mode requires complete response).

**Multimodal**: Variables ending with `_image` or `_image_base64` are automatically
included as image content in the LLM request.

---

### `data_connector` — Data Source Operations

Connects to external data sources (databases, caches, object storage, APIs).

```yaml
- id: query_users
  type: data_connector
  config:
    data_source_id: main_db              # Required. References data_sources or Engine-level source
    operation:                           # Required. Kind-specific operation config
      operation: query                   # Operation type (see below)
      sql: "SELECT * FROM users WHERE id = {userId}::uuid"
      output_fields:                     # Optional. Define expected output fields
        - name: nickname
          type: string
    variables:                           # Required for parameterized operations
      userId: start.user_id
```

#### PostgreSQL Operations

```yaml
# query — Raw SQL (recommended)
operation:
  operation: query
  sql: "SELECT * FROM users WHERE id = {userId}::uuid"
  output_fields:
    - name: nickname
      type: string

# insert
operation:
  operation: insert
  table: users
  data:
    name: "{userName}"
    email: "{userEmail}"

# select
operation:
  operation: select
  table: users
  columns: ["id", "name"]
  where: "id = {userId}::uuid"
  limit: 10

# update
operation:
  operation: update
  table: users
  data:
    name: "{newName}"
  where: "id = {userId}::uuid"

# delete
operation:
  operation: delete
  table: users
  where: "id = {userId}::uuid"
```

**Output**: `{node_id}.rows` (array), `{node_id}.count` (number), plus first row fields flattened to top level.

#### Redis Operations

```yaml
# get
operation:
  operation: get
  key_template: "user:{userId}"

# set
operation:
  operation: set
  key_template: "user:{userId}"
  value_template: "{userData}"
  ttl: 3600                              # Optional. Seconds

# del
operation:
  operation: del
  key_template: "user:{userId}"

# publish
operation:
  operation: publish
  key_template: "channel:{channelId}"
  value_template: "{message}"

# append_history (Redis JSON array append)
operation:
  operation: append_history
  key_template: "chat:{userId}:{characterId}"
  json_path: ".messages"
  value_template: "{newMessage}"
```

**Output**: `{node_id}.value` (for get), `{node_id}.result` (for set/del/publish).

#### Qdrant (Vector DB) Operations

```yaml
# search — Similarity search
operation:
  operation: search
  collection: "{collectionName}"
  query: "{searchText}"                  # Text to embed
  embedding_provider: dashscope          # Engine-registered embedding provider
  embedding_model: text-embedding-v3
  limit: 5
  score_threshold: 0.7                   # Optional

# batch_upsert — Insert/update vectors
operation:
  operation: batch_upsert
  collection: "{collectionName}"
  texts: "{textsArray}"                  # Array of texts to embed and store
  embedding_provider: dashscope
  embedding_model: text-embedding-v3
  payloads: "{payloadsArray}"            # Optional. Metadata for each vector
```

**Output**: `{node_id}.results` (array of matches with score and payload).

#### MinIO/S3 Operations

```yaml
# upload
operation:
  operation: upload
  bucket: my-bucket
  key_template: "uploads/{userId}/{fileName}"
  content: "{fileContent}"               # Base64 encoded
  content_type: "audio/wav"

# download
operation:
  operation: download
  bucket: my-bucket
  key_template: "uploads/{fileKey}"

# delete
operation:
  operation: delete
  bucket: my-bucket
  key_template: "uploads/{fileKey}"
```

**Output**: `{node_id}.url` (for upload), `{node_id}.content` (for download).

#### REST API Operations

```yaml
operation:
  method: POST                           # GET | POST | PUT | DELETE | PATCH
  path: "/api/v1/users"
  headers:
    Authorization: "Bearer {token}"
  body:
    name: "{userName}"
  body_type: json                        # json | form | raw
```

**Output**: `{node_id}.status`, `{node_id}.body`, `{node_id}.headers`.

#### GraphQL Operations

```yaml
operation:
  query: |
    query GetUser($id: ID!) {
      user(id: $id) { name email }
    }
  graphql_variables:
    id: "{userId}"
```

**Output**: `{node_id}.data`, `{node_id}.errors`.

---

### `response` — Output Node

Every workflow has exactly one response node. Maps final output from state.

```yaml
- id: response
  type: response
  config:
    output:
      result_field: "some_node.field"
      another: "other_node.text"
```

**Output**: The response node's output becomes the workflow's `ExecutionResult.output`.

---

### `checkpoint` — Pause/Resume Point

Pauses workflow execution and returns serialized state for later resumption.

```yaml
- id: ask_user
  type: checkpoint
  config:
    output:
      question: "analyze.clarification_question"
```

The engine returns `ExecutionStatus::Paused` with a `CheckpointState`.
Resume with `engine.resume(workflow_name, checkpoint_state, "ask_user", new_input)`.

---

### `supervisor` — Agent Planning Loop

Implements a plan-execute loop: LLM generates a todo list, engine executes tools, repeats.

```yaml
- id: agent
  type: supervisor
  config:
    provider: qwen                       # Required
    model: qwen-plus                     # Required
    template: planner_template           # Required. Must produce JSON todo list
    tools:                               # Required. Available tools
      - name: search_web
        description: "Search the web for information"
        ref: search_subgraph             # References a subgraph ID
      - name: calculate
        description: "Perform mathematical calculations"
        ref: calc_subgraph
    max_iterations: 10                   # Optional. Default: engine default
    max_retry_per_tool: 3                # Optional. Default: 3
    parallel: false                      # Optional. Execute tools in parallel
    timeout: 300000                      # Optional. Milliseconds
    max_result_length: 10000             # Optional. Characters
    fail_on_tool_count: 5                # Optional. Failures before exit
    summarizer_template: summary_tmpl    # Optional. Post-process results
```

**Exit edges** from supervisor use `exit_reason`:

```yaml
edges:
  - from: agent
    to: success_handler
    exit_reason: todo_empty
  - from: agent
    to: timeout_handler
    exit_reason: timeout
  - from: agent
    to: fallback
    exit_reason: default                 # Catches any exit reason
```

**Exit reasons**: `todo_empty`, `timeout`, `max_iterations`, `max_result_length`, `tool_failures`, `default`.

**Output**: `{node_id}.text` (accumulated results), `{node_id}.iterations` (loop count).

---

### `workflow` — Sub-Workflow Call

Executes another workflow as a child.

```yaml
- id: compress
  type: workflow
  config:
    workflow: compress_history            # Required. Name of another workflow
    variables:                           # Optional. Input mapping
      user_id: start.user_id
      history: load_history.rows
    output:                              # Optional. Output mapping
      summary: "compressed_summary"      # Maps sub-workflow output to parent field
```

**Output**: Fields defined in the `output` mapping.

---

### Custom Nodes

Any `type` not matching the built-in types is treated as a custom node.
Custom nodes must be registered in the Engine builder at startup.

```yaml
- id: speech_to_text
  type: stt                              # Custom type registered via Engine::builder()
  config:
    variables:
      audio_data: start.user_voice_data
```

Custom nodes receive the full `NodeConfig` and `ExecutionState`.

---

## Edges

Edges define the execution graph. Nodes execute when all incoming edge sources have completed.

```yaml
edges:
  # Unconditional edge
  - from: start
    to: load_data

  # Conditional edge
  - from: analyze
    to: search
    condition: "analyze.has_enough_info == true"

  # Default edge (taken when no conditional edges match)
  - from: analyze
    to: ask_more
    is_default: true

  # Loop with protection
  - from: refine
    to: check
    max_iterations: 5

  # Supervisor exit edge
  - from: agent
    to: handler
    exit_reason: todo_empty
```

### Condition Expression Language

```
# Comparison operators
node.field == "value"
node.field != 0
node.field > 10
node.field >= 5
node.field < 100
node.field <= 50

# Logical operators
condition1 && condition2
condition1 || condition2
!condition

# Built-in functions
empty(node.field)        # true if null, "", [], or {}
exists(node.field)       # true if not null
len(node.field)          # returns length (> 0 for truthy check)

# Grouping
(a == 1 || b == 2) && c == 3
```

---

## Templates

Templates define LLM prompts with Handlebars syntax.

```yaml
templates:
  my_template:
    system_prompt: "You are a helpful assistant."
    user_prompt: |
      {{#if history}}
      Previous conversation:
      {{history}}
      {{/if}}

      User: {{user_message}}
    config:
      temperature: 0.7
      max_tokens: 2048
      top_p: 1.0
      enable_search: false               # For providers that support web search
    output_schema:                        # Optional. Forces structured JSON output
      - name: answer
        type: string
        description: "The answer to the question"
      - name: confidence
        type: number
        description: "Confidence score 0-1"
```

### Template Config Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `temperature` | float | 0.7 | Sampling temperature (0.0 - 2.0) |
| `max_tokens` | int | 2048 | Maximum tokens to generate |
| `top_p` | float | null | Top-p nucleus sampling |
| `top_k` | int | null | Top-k sampling |
| `thinking_budget` | int | null | Extended thinking token budget |
| `enable_search` | bool | false | Enable web search (provider-dependent) |

### Output Schema Types

`string`, `number`, `boolean`, `string[]`, `number[]`, `object`, `object[]`

Templates can also be loaded from external files via `Engine::builder().prompts_dir("./prompts")`.
File-based templates are referenced by filename (without extension).

---

## Subgraphs

Subgraphs are mini-workflows used as tools by supervisor nodes.

```yaml
subgraphs:
  search_subgraph:
    nodes:
      - id: start
        type: start
      - id: search
        type: data_connector
        config:
          data_source_id: search_api
          operation:
            method: GET
            path: "/search?q={query}"
          variables:
            query: start.query
      - id: response
        type: response
        config:
          output:
            result: search.body
    edges:
      - from: start
        to: search
      - from: search
        to: response
```

---

## Parallel Execution

Nodes execute as soon as their input dependencies are satisfied (not layer-based).
Multiple nodes with the same source can run in parallel:

```yaml
edges:
  - from: start
    to: load_user          # These three run in parallel
  - from: start
    to: load_history       # because they all depend only on start
  - from: start
    to: get_time
  - from: load_user
    to: generate           # This waits for all three
  - from: load_history
    to: generate
  - from: get_time
    to: generate
```

---

## Data Source Priority

Data sources can be defined at two levels:

1. **Engine-level**: Registered via `Engine::builder().pg_pool(id, pool)` or similar
2. **Workflow-level**: Defined in the `data_sources` section of the YAML

**Priority**: Workflow-level definitions override Engine-level ones with the same `source_id`.

For development/testing, include `data_sources` in your YAML so the workflow is self-contained.
For production, register data sources at Engine level and omit them from YAML.

---

## Complete Example

```yaml
version: "1.0"
name: "simple_chat"
description: "A simple chatbot workflow"

input:
  fields:
    - name: user_id
      type: string
      required: true
    - name: message
      type: string
      required: true

data_sources:
  main_db:
    name: "Main Database"
    kind: postgresql
    config:
      url: "${DATABASE_URL}"

nodes:
  - id: start
    type: start
    config:
      output:
        user_id: "input.user_id"
        message: "input.message"

  - id: load_history
    type: data_connector
    config:
      data_source_id: main_db
      operation:
        operation: query
        sql: "SELECT role, content FROM messages WHERE user_id = {userId}::uuid ORDER BY created_at DESC LIMIT 20"
        output_fields:
          - name: role
            type: string
          - name: content
            type: string
      variables:
        userId: start.user_id

  - id: chat
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: chat_template
      variables:
        user_message: start.message
        history:
          from: load_history.rows
          default: "[]"

  - id: save_message
    type: data_connector
    config:
      data_source_id: main_db
      operation:
        operation: query
        sql: |
          INSERT INTO messages (user_id, role, content)
          VALUES ({userId}::uuid, 'assistant', {response})
      variables:
        userId: start.user_id
        response: chat.text

  - id: response
    type: response
    config:
      output:
        reply: "chat.text"

edges:
  - from: start
    to: load_history
  - from: load_history
    to: chat
  - from: chat
    to: save_message
  - from: save_message
    to: response

templates:
  chat_template:
    system_prompt: "You are a helpful assistant."
    user_prompt: |
      {{#if history}}
      Chat history:
      {{#each history}}
      {{this.role}}: {{this.content}}
      {{/each}}
      {{/if}}

      User: {{user_message}}
    config:
      temperature: 0.7
      max_tokens: 1024
```
