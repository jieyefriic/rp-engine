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

providers:                               # Optional. YAML-level provider API keys
  provider_id:                           # Overrides Engine-level providers
    api_key: "${OPENAI_API_KEY}"         # Supports ${ENV_VAR} expansion
    base_url: "https://custom.endpoint"  # Optional. For proxies or custom endpoints
    kind: openai_compatible              # Optional. Defaults to provider_id name
    default_model: "my-model"            # Optional. For openai_compatible

data_sources:                            # Optional. Workflow-level data source definitions
  source_id:                             # These override Engine-level sources with the same ID
    name: "Human Name"
    kind: postgresql                     # postgresql | mysql | mongodb | redis | qdrant | s3 | minio | rest_api | graphql
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

subgraphs:                               # Optional. For supervisor, iterator, and generate tool calling
  subgraph_id:
    nodes: [...]
    edges: [...]

mcp_servers:                             # Optional. MCP server definitions
  server_id:
    url: "https://mcp.example.com/mcp"  # Required. Streamable HTTP endpoint
    auth:                                # Optional. Authentication
      type: bearer                       # "bearer" or "api_key"
      token: "${MCP_TOKEN}"              # Supports ${ENV_VAR} expansion

harness:                                 # Optional. Workflow-level instructions for all generate nodes
  instructions: "..."                    # Inline instruction text
  files: ["rules/project.md"]           # External .md file paths
  memory_file: "harness_memory.json"    # Optional. Persistent memory (read at start, updated after execution)
  writeback:                             # Optional. Post-execution memory writeback via LLM
    provider: deepseek                   # LLM provider for writeback call
    model: deepseek-chat                 # Model (can be cheap/fast)
    prompt: "..."                        # Optional. Custom writeback prompt
    memory_char_limit: 2200             # Optional. Max chars for memory file (default: 2200)
```

### Harness Instructions

Harness provides workflow-level instructions that are injected into **every generate node's system_prompt**, before the template's own system_prompt. This is similar to Claude Code's `CLAUDE.md` — always-present project rules.

```yaml
harness:
  instructions: |
    You are a professional data analysis assistant.
    Rules:
    1. Always respond in Chinese
    2. Use precise technical terminology
    3. Include data sources in your analysis
  files:
    - "rules/output_format.md"
    - "rules/company_style.md"
  memory_file: "harness_memory.txt"
  writeback:
    provider: deepseek
    model: deepseek-chat
    memory_char_limit: 2200
    prompt: |
      Review the execution results and update memory with
      key user preferences and important context.
```

**Behavior:**
- `instructions`: Inline text, injected as-is
- `files`: External file paths, contents read and concatenated (separated by `---`)
- `memory_file`: Read at workflow start, appended to instructions as `## Memory` section. Content is a frozen snapshot for the current run.
- `writeback`: Post-execution LLM call that reviews workflow output + current memory and writes updated memory back to `memory_file`. See **Writeback** below.

**System prompt composition order (highest priority first):**
```
harness instructions
---
skill metadata (from skill_set nodes)
---
template system_prompt
```

If no template has a system_prompt, harness instructions become the sole system_prompt. If no harness is defined, behavior is identical to before (backward compatible).

### Writeback

When `harness.writeback` is configured, the engine performs a dedicated LLM call **after** workflow execution completes. This call reviews the workflow's final output together with the current memory content and produces updated memory, which is written back to `memory_file`.

```yaml
harness:
  memory_file: "harness_memory.txt"
  writeback:
    provider: deepseek        # Required. LLM provider for the writeback call
    model: deepseek-chat      # Required. Model (a cheap/fast model is recommended)
    memory_char_limit: 2200   # Optional. Max chars for memory file (default: 2200)
    prompt: |                 # Optional. Custom prompt (sensible default if omitted)
      Review the execution results and update memory with
      key user preferences and important context.
```

**Writeback fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `provider` | Yes | LLM provider for the writeback call |
| `model` | Yes | Model to use (cheap/fast model recommended) |
| `memory_char_limit` | No | Maximum characters for the memory file. Default: 2200 (~800 tokens, following Hermes Agent convention) |
| `prompt` | No | Custom writeback prompt. Default focuses on: user preferences, habits, key decisions, important context |

**Writeback behavior:**
- Fires only after successful workflow execution
- **Best-effort**: writeback failure does NOT fail the workflow (logged as warning)
- Current run uses a **frozen snapshot** of memory — writeback changes take effect on the next run
- Memory file is truncated to `memory_char_limit` to prevent unbounded growth
- Requires `memory_file` to be set (writeback without memory_file is a no-op)

**Design intent:** Harness constrains LLM behavior (`instructions` = "you must/cannot do X") and learns user habits (`writeback` = preferences/patterns accumulated over runs). Everything else (skills, tools, context) is about expanding capabilities.

---

## Variable Binding

All nodes that accept input from other nodes use the `variables` field.
There is exactly **one syntax** for variable binding:

```yaml
variables:
  # Form 1: Simple reference (required — blocks until source produces value)
  varName: "node_id.field"

  # Form 2: With default value (see Default Semantics below)
  varName:
    from: "node_id.field"
    default: ""

  # Form 3: Multiple sources (first available wins, blocks until at least one produces)
  varName:
    - "node_a.field"
    - "node_b.field"

  # Form 4: Multiple sources with default
  varName:
    from:
      - "node_a.field"
      - "node_b.field"
    default: null

  # Form 5: Literal constant (no dot → treated as value, not a state path)
  limit: "2"                    # → Number(2)
  enabled: "true"               # → Bool(true)
  label: "hello"                # → String("hello")
```

### Literal vs Path

The engine distinguishes sources by the presence of `.`:

| Source | Contains `.` | Interpretation |
|--------|:-----------:|----------------|
| `start.user_id` | yes | State path → read from node output |
| `"2"` | no | Literal → `Number(2)` |
| `"true"` | no | Literal → `Bool(true)` |
| `"hello"` | no | Literal → `String("hello")` |

Literal type parsing priority: integer → float → boolean → string.

### Default Semantics (Dead Path Aware)

`default` means: **"use this value when the source is confirmed unable to produce."**

The engine waits until every source node is **settled** — either completed execution or confirmed
unreachable (dead path) — before falling back to the default. This prevents two failure modes:

1. **Premature execution**: Without dead-path awareness, a default would fire immediately,
   causing the node to run before a live source finishes.
2. **Infinite hang**: Without defaults, a variable referencing a node on an untaken conditional
   branch would block forever.

**Example — conditional image generation:**
```yaml
# Image is generated only when quota is available.
# When quota is exhausted, gen_image never executes → engine marks it dead → default kicks in.
variables:
  imageUrl:
    from: gen_image.url
    default: ""
```

If a variable references a dead-path node **without** a default, the engine reports a clear error:
> Node 'save' variable 'imageUrl' references 'gen_image.url' which is on an unreachable path and has no default value.

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

**Multimodal (images)**: Use the `images` field to pass image data to vision-capable models.
Each entry is a variable binding that resolves to a string in one of three formats:
- **HTTP/HTTPS URL**: `https://example.com/photo.jpg` — passed directly to providers that support it
- **Data URI**: `data:image/png;base64,iVBOR...` — media type extracted, base64 data sent
- **Raw base64**: Pure base64 string — media type auto-detected via magic bytes (JPEG/PNG/WebP/GIF)

```yaml
- id: analyze
  type: generate
  config:
    provider: openai
    model: gpt-4o
    template: analyze_tpl
    variables:
      user_message: start.user_content
    images:
      - start.user_image        # single binding
```

The engine validates at build time that the selected provider/model supports vision.
Gemini does not support HTTP image URLs (only GCS URIs); URL images will be skipped with a warning.

**Skill Sets**: Generate nodes can reference `skill_set` nodes for progressive disclosure of knowledge.

```yaml
- id: helper
  type: generate
  config:
    provider: deepseek
    model: deepseek-chat
    template: helper_tpl
    skill_sets: ["coding_skills"]        # References skill_set node IDs
    variables:
      question: "start.question"
```

When `skill_sets` is configured:
1. The engine reads resolved skill metadata from each referenced `skill_set` node's state
2. Injects an `## Available Skills` section into the system_prompt middle layer (between harness and template)
3. Registers two built-in tools: `load_skill` (loads full skill content) and `load_skill_reference` (loads L3 reference files)
4. The LLM sees skill metadata (name + description), calls `load_skill` to get full content, and optionally calls `load_skill_reference` for referenced resources

User-defined tools **cannot** be named `load_skill` or `load_skill_reference` — the engine rejects these at build time.

**MCP Tool Sets**: Generate nodes can reference `mcp_tools` nodes for LLM-driven MCP tool calling.

```yaml
- id: agent
  type: generate
  config:
    provider: deepseek
    model: deepseek-chat
    template: agent_tpl
    mcp_tool_sets: ["wiki_tools"]        # References mcp_tools node IDs
    variables:
      question: "start.question"
```

When `mcp_tool_sets` is configured, MCP tools are injected into the generate node's tool definitions alongside user subgraph tools and skill tools. The LLM can call MCP tools autonomously during the tool calling loop; calls are dispatched to the corresponding MCP server.

**Native Tool Calling**: Generate nodes can use LLM function calling to invoke tools backed by subgraphs.
The LLM decides when to call tools; results are fed back in a multi-round loop.

```yaml
- id: agent
  type: generate
  config:
    provider: openai
    model: gpt-4o
    template: agent_prompt
    variables:
      user_input: start.query
    tools:                               # Optional. Native function calling
      - name: search_web
        description: "Search the web for information"
        ref: sg_search                   # Subgraph reference (see Subgraphs section)
        parameters:                      # JSON Schema for tool arguments
          type: object
          properties:
            query: { type: string, description: "Search query" }
          required: ["query"]
      - name: calculate
        description: "Evaluate a math expression"
        ref: sg_calc
    max_tool_rounds: 5                   # Safety limit (default: 10)
```

**Execution flow**: LLM call → if LLM returns tool_calls → execute each tool's subgraph →
send results back to LLM → repeat until LLM gives final text response or max_tool_rounds reached.

Streaming is automatically disabled when tools are configured (multi-round requires complete responses).

If a model doesn't support native function calling (`functionCalling: false` in model registry),
the `tools` field is silently ignored.

**Error Handling**: Any node can configure retry, timeout, and failure behavior.

```yaml
- id: risky_call
  type: generate
  config:
    provider: openai
    model: gpt-4o
    template: my_prompt
    error_handling:                       # Optional. Applies to ANY node type
      retry: 3                           # Number of retry attempts (default: 0)
      retry_delay_ms: 2000               # Delay between retries in ms (default: 1000)
      timeout_ms: 30000                  # Per-node execution timeout in ms
      on_exhausted: continue             # "fail" (default) | "continue"
```

When `on_exhausted: continue`, the node's outputs contain `_error: true` and `_error_message: "..."`.
Use `on_error: true` edges (see Edges section) to route to fallback nodes.

**Capability Masking**: The engine automatically masks ChatOptions parameters based on model capabilities
from `providers.json`:

| Parameter | Masked when |
|---|---|
| `json_output` | model.json = false → removed, relies on prompt injection + robust extraction |
| `top_p` | provider.sampling.supportsTopP = false → removed |
| `top_k` | provider.sampling.supportsTopK = false → removed |
| `temperature` | Clamped to model/provider maxTemperature |
| `thinking_budget` | model.thinking = "none" → removed |
| `thinking + json` | model.thinkingJsonCompat = None → thinking removed, JSON kept |
| `tools` | model.functionCalling = false → removed |

This ensures workflows are portable across providers without manual parameter tuning.

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

**Note**: PostgreSQL DECIMAL/NUMERIC columns require explicit casting in parameterized queries:
`price::float8 >= {min_price}` (because the engine binds params as INT8 or FLOAT8, not NUMERIC natively).

#### MySQL / MariaDB Operations

**kind**: `mysql` or `mariadb`

```yaml
data_sources:
  my_mysql:
    name: "My MySQL"
    kind: mysql
    config:
      host: "localhost"
      port: 3306
      username: "user"
      password: "${MYSQL_PASSWORD}"
      database: "mydb"
```

Supports the same operations as PostgreSQL: `query`, `select`, `insert`, `update`, `delete`.

MySQL differences from PostgreSQL:
- **No RETURNING clause**: `insert` returns `last_insert_id` and `count` instead. If `returning` is specified, a follow-up SELECT is executed automatically.
- **Backtick quoting**: Column and table names use `` ` `` instead of `"`.
- **Same {paramName} binding** syntax for parameterized queries.

#### MongoDB Operations

**kind**: `mongodb` or `mongo`

```yaml
data_sources:
  my_mongo:
    name: "Analytics DB"
    kind: mongodb
    config:
      url: "mongodb://host:27017"
      database: "mydb"
```

**Operations:**

```yaml
# find — query multiple documents
operation:
  operation: find
  collection: "users"
  filter: { status: "active", age: { "$gte": 18 } }  # MongoDB query syntax
  sort: { created_at: -1 }
  limit: 100
  skip: 0
  projection: { name: 1, email: 1 }

# find_one — single document
operation:
  operation: find_one
  collection: "users"
  filter: { _id: "{user_id}" }

# insert_one
operation:
  operation: insert_one
  collection: "logs"
  data: { message: "{log_msg}", level: "info", timestamp: "{ts}" }

# insert_many
operation:
  operation: insert_many
  collection: "events"
  data:
    - { type: "click", target: "button_a" }
    - { type: "view", target: "page_b" }

# update_one
operation:
  operation: update_one
  collection: "users"
  filter: { _id: "{user_id}" }
  update: { status: "premium", tier: 2 }        # Auto-wrapped in $set if no $ operators

# update_many — same as update_one but affects all matching docs

# delete_one / delete_many
operation:
  operation: delete_one
  collection: "logs"
  filter: { source: "rp-engine-test" }

# aggregate — pipeline
operation:
  operation: aggregate
  collection: "orders"
  pipeline:
    - { "$match": { status: "completed" } }
    - { "$group": { _id: "$category", total: { "$sum": "$amount" } } }
    - { "$sort": { total: -1 } }

# count
operation:
  operation: count
  collection: "users"
  filter: { status: "active" }
```

**Output**: `find`/`aggregate` return `{node_id}.rows` (array) and `{node_id}.count`.
`find_one` returns `{node_id}.document` and `{node_id}.exists` (boolean).
`insert_one` returns `{node_id}.inserted_id`. `update_one` returns `{node_id}.matched_count` and `{node_id}.modified_count`.

**Variable resolution**: All string values in `filter`, `data`, `update`, and `pipeline` support `{paramName}` references that resolve from the node's `variables` map.

#### Redis Operations

**Connection pooling**: Register Redis connections at engine build time for connection reuse:

```rust
Engine::builder()
    .redis("cache", "redis://localhost:6379/0")   // data_source_id -> URL
    .redis("sessions", "redis://redis2:6379/1")
    .build()
    .await?;
```

If no pool is registered for a `data_source_id`, an ad-hoc connection is created from the data source config (slower, no reuse).

**Basic operations**:

```yaml
# get — read a key, optionally extract fields via output_schema
operation:
  operation: get
  key_template: "user:{userId}"
  output_schema:                           # Optional. Extract typed fields
    - name: chat_history
      type: array
    - name: summary
      type: string

# set — write a key with optional TTL
operation:
  operation: set
  key_template: "user:{userId}"
  value_template: "{userData}"
  ttl: 3600                              # Optional. Seconds

# del — delete a key
operation:
  operation: del
  key_template: "user:{userId}"

# publish — publish a message to a Redis channel
operation:
  operation: publish
  key_template: "channel:{channelId}"
  value_template: "{message}"
```

**Output** (get): `{node_id}.value`, `{node_id}.exists`, plus `output_schema` fields.
**Output** (set/publish): `{node_id}.ok`.
**Output** (del): `{node_id}.ok`, `{node_id}.deleted`.

**`value` vs `value_template`**: All write operations (set, json_append, json_merge) accept either:

- `value_template` — flat string template: `"user:{userId}"` → resolves to string
- `value` — structured YAML with recursive `{var}` resolution (type-preserving)

When a string leaf is exactly `"{varName}"` and the variable holds a non-string type (array, object, number, bool), the raw value is injected directly without stringification.

```yaml
# value_template: for simple/flat values
value_template: "{topic}"

# value: for structured objects (no transform needed)
value:
  - role: "user"
    content: "{userMessage}"
    type: "{messageType}"
  - role: "assistant"
    content: "{assistantMessage}"
```

If both `value` and `value_template` are present, `value` takes priority.

**Atomic JSON operations** (via Lua scripts, zero race conditions):

```yaml
# json_append — atomically append element(s) to a JSON array field
# value is an array → elements are spread; single value → pushed
operation:
  operation: json_append
  key_template: "chat:{userId}:{characterId}"
  path: "chat_history"                   # JSON field ("" = root must be array)
  value:                                 # Structured — builds messages directly
    - role: "user"
      content: "{userMessage}"
      type: "{messageType}"
    - role: "assistant"
      content: "{assistantMessage}"
  ttl: 86400                             # Optional

# json_append with flat value
operation:
  operation: json_append
  key_template: "user:{userId}:topics"
  path: ""                               # Root is array
  value_template: "{topic}"              # Simple string value
  ttl: 86400

# json_merge — atomically shallow-merge an object into a JSON field
operation:
  operation: json_merge
  key_template: "travel_meta:{userId}"
  path: ""                               # "" = merge into root object
  value:                                 # Structured merge
    last_seen: "{timestamp}"
    message_count: "{count}"
  ttl: 86400

# json_delete — atomically delete a field from a JSON object
operation:
  operation: json_delete
  key_template: "chat:{userId}:{characterId}"
  path: "summary"                        # Field to remove (required)
  ttl: 86400                             # Optional. Refreshes TTL
```

**Output** (json_append): `{node_id}.ok`, `{node_id}.length` (new array length).
**Output** (json_merge): `{node_id}.ok`.
**Output** (json_delete): `{node_id}.ok`, `{node_id}.deleted`.

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

#### S3 Operations

Data source config supports optional `public_endpoint` for presigned URL generation:

```yaml
data_sources:
  storage:
    kind: s3
    config:
      endpoint: "http://localhost:9000"
      public_endpoint: "https://s3.example.com"  # Optional. Used by presigned_url
      access_key: "..."
      secret_key: "..."
      region: "us-east-1"                        # Optional. Default: us-east-1
```

```yaml
# get_object — download object, auto-decodes text/json, base64 for binary
operation:
  operation: get_object
  bucket: my-bucket
  key: "uploads/{fileKey}"

# put_object — upload base64 data, auto-detect content type from magic bytes
operation:
  operation: put_object
  bucket: "{userId}"                     # Supports {var} template from variables
  prefix: uploads                        # Optional. Default: "uploads"
  content_type: "auto"                   # Optional. "auto" or MIME type. Default: auto-detect

# list_objects — list objects with optional prefix filter
operation:
  operation: list_objects
  bucket: my-bucket
  prefix: "uploads/"                     # Optional
  max_keys: 100                          # Optional. Default: 1000

# delete_object — delete a single object
operation:
  operation: delete_object
  bucket: my-bucket
  key: "uploads/{fileKey}"

# head_object — check existence + metadata without downloading
operation:
  operation: head_object
  bucket: my-bucket
  key: "uploads/{fileKey}"

# presigned_url — generate temporary read URL
operation:
  operation: presigned_url
  bucket: my-bucket
  key: "uploads/{fileKey}"
  expires_in: 3600                       # Optional. Seconds. Default: 3600

# copy_object — server-side copy between buckets/keys
operation:
  operation: copy_object
  source_bucket: src-bucket
  source_key: "original/{srcKey}"
  bucket: dest-bucket
  key: "copies/{destKey}"
```

**Outputs**:
- `get_object`: `data`, `content_type`, `size`, `last_modified`
- `put_object`: `url`, `bucket`, `key`
- `list_objects`: `objects` (array), `truncated` (bool)
- `delete_object`: `deleted` (bool)
- `head_object`: `exists` (bool), `content_type`, `size`, `last_modified`
- `presigned_url`: `url`, `expires_in`
- `copy_object`: `bucket`, `key`

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

### `transform` — Rhai Script Computation

Runs a Rhai script file for pure computation (no I/O). Scripts are loaded from
`Engine::builder().transforms_dir("./transforms")` at startup.

```yaml
- id: build_payload
  type: transform
  config:
    file: "build_payload"              # Required. Script name (loads transforms/build_payload.rhai)
    variables:                         # Optional. Injected as Rhai global variables
      user_name: start.name
      history: load_history.rows
```

**Script contract:**
- Resolved `variables` are injected as Rhai global variables
- Additionally, `_vars` is injected as a Map containing all resolved variables (for generic scripts)
- `serde_json::Value` ↔ Rhai `Dynamic` auto-conversion
- Script's last expression = node output
- If return is a Map → each key becomes an output field
- If return is not a Map → wrapped as `{"result": value}`

**Example script** (`transforms/build_payload.rhai`):
```rhai
// Variables 'user_name' and 'history' are injected from the node config
let greeting = "Hello, " + user_name;
let msg_count = history.len();
let unique_history = unique(history);

// Return a map — each key becomes a node output field
#{
    "greeting": greeting,
    "message_count": msg_count,
    "unique_history": unique_history,
    "timestamp": now(),
    "request_id": uuid()
}
```

**`_vars` — generic variable aggregation** (`transforms/build_json.rhai`):
```rhai
// _vars is a map of ALL resolved variables — works for any variable combination
#{ "json_string": json_encode(_vars), "json_object": _vars }
```

**Built-in functions:**

| Function | Returns | Description |
|----------|---------|-------------|
| `now()` | `string` | Current timestamp (Beijing time, `YYYY-MM-DD HH:MM:SS`) |
| `uuid()` | `string` | Random UUID v4 |
| `json_encode(value)` | `string` | JSON-serialize any value |
| `unique(array)` | `array` | Deduplicate array (preserves order) |

**Sandboxing:** Rhai is a pure computation engine — no file I/O, no network access,
no system calls. Scripts are limited to 100,000 operations, 1MB strings, and 10,000 array/map entries.

**Output**: Fields from the returned map, or `{node_id}.result` for non-map returns.

---

### `search` — _(Removed)_

Search node type has been removed. Search capability will be available as a built-in tool for generate nodes in a future release. The search provider infrastructure (Tavily, Brave, Bocha, Perplexity) is retained internally for this purpose.

YAML files using `type: search` will fail with `CustomNodeNotFound`.

---

### `mcp` — Explicit MCP Tool Call

Calls a specific tool on an MCP (Model Context Protocol) server and returns the result. The MCP client uses Streamable HTTP transport with JSON-RPC 2.0.

```yaml
- id: wiki_result
  type: mcp
  config:
    server: deepwiki                       # Required. References mcp_servers section
    tool: read_wiki_structure              # Required. MCP tool name
    variables:                             # Optional. Tool arguments
      repoName: "anthropics/anthropic-cookbook"
```

**Config fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `server` | string | **Yes** | MCP server ID from `mcp_servers` top-level section |
| `tool` | string | **Yes** | Tool name to invoke on the server |
| `variables` | map | No | Tool arguments (variable bindings, same syntax as other nodes) |

**Output fields:**

| Field | Type | Description |
|-------|------|-------------|
| `{node_id}.result` | any | Tool call result (JSON value from MCP server) |
| `{node_id}._timing_ms` | number | Execution time in milliseconds |
| `{node_id}._error` | bool | Present and `true` on failure |
| `{node_id}._error_message` | string | Error description (on failure) |

**MCP server definition** (in top-level `mcp_servers`):

```yaml
mcp_servers:
  deepwiki:
    url: "https://mcp.deepwiki.com/mcp"
  private_server:
    url: "https://internal.example.com/mcp"
    auth:
      type: bearer                         # "bearer" or "api_key"
      token: "${MCP_TOKEN}"
```

**Auth types:**
- `bearer` — sends `Authorization: Bearer <token>` header
- `api_key` — sends `X-API-Key: <token>` header

**Protocol details:**
- Transport: Streamable HTTP (POST to server URL)
- Wire format: JSON-RPC 2.0
- Handles both plain JSON and SSE (`text/event-stream`) responses
- Session management via `Mcp-Session-Id` header

**Example — call DeepWiki (no auth needed):**

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

### `mcp_tools` — MCP Tool Discovery for LLM

Connects to an MCP server, discovers available tools via `tools/list`, and outputs tool definitions to state. These tools are consumed by `generate` nodes through the `mcp_tool_sets` config field, enabling LLM-driven MCP tool calling.

```yaml
- id: wiki_tools
  type: mcp_tools
  config:
    server: deepwiki                       # Required. References mcp_servers section
```

**Config fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `server` | string | **Yes** | MCP server ID from `mcp_servers` top-level section |

**Output fields:**

| Field | Type | Description |
|-------|------|-------------|
| `{node_id}.tools` | array | Array of MCP tool definitions (name, description, inputSchema) |
| `{node_id}.server` | string | Server ID (for routing tool calls) |
| `{node_id}._tool_count` | number | Number of tools discovered |

**Connection to generate nodes:**

Generate nodes consume MCP tools via the `mcp_tool_sets` config field (same pattern as `skill_sets`). The `mcp_tools` node must be connected via edges to the generate node.

```yaml
nodes:
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
      mcp_tool_sets: ["wiki_tools"]        # References mcp_tools node IDs
      variables:
        question: "start.question"

edges:
  - from: wiki_tools
    to: agent
```

When `mcp_tool_sets` is configured:
1. The engine reads tool definitions from each referenced `mcp_tools` node's state
2. Injects MCP tools into the generate node's tool_defs alongside user subgraph tools and skill tools
3. During the tool calling loop, the LLM can autonomously call MCP tools
4. MCP tool calls are dispatched to the corresponding MCP server

---

### `iterator` — Batch Array Processing

Processes an array by running a subgraph for each element. Supports parallel execution with concurrency control.

```yaml
- id: batch_summarize
  type: iterator
  config:
    items: "upstream.documents"          # Required. Variable binding → must resolve to array
    item_var: "item"                     # Variable name injected per iteration (default: "item")
    index_var: "index"                   # Optional. 0-based index variable
    subgraph: process_one                # Required. Subgraph ID from subgraphs section
    parallel: true                       # Execute iterations concurrently (default: false)
    max_concurrency: 5                   # Max parallel iterations (default: 5)
    max_iterations: 1000                 # Safety limit on array length (default: 1000)
    on_error: collect                    # "fail_fast" (default) | "collect"
    variables:                           # Optional. Shared variables available to all iterations
      context: "start.context"
```

**Subgraph input**: Each iteration creates an isolated `ExecutionState` with:
- `input.{item_var}` → current array element
- `input.{index_var}` → current 0-based index (if configured)
- `input.{shared_var}` → all shared variables

**Output fields:**
- `{node_id}.results` — Array of subgraph outputs (same order as input)
- `{node_id}.count` — Total items processed
- `{node_id}.success_count` — Successful iterations
- `{node_id}.error_count` — Failed iterations
- `{node_id}.errors` — Array of `{index, error}` (only in collect mode)
- `{node_id}._usage` — Aggregated token usage across all iterations

**Error modes:**
- `fail_fast`: Stop on first iteration error, return `IteratorItemFailed` error
- `collect`: Continue all iterations, collect errors in `errors` array. Failed items appear as `{"_error": "..."}` in results.

**Example subgraph:**

```yaml
subgraphs:
  process_one:
    nodes:
      - id: gen
        type: generate
        config:
          provider: qwen
          model: qwen-plus
          template: summarize_tpl
          variables:
            doc: "input.item"
            idx: "input.index"
      - id: result
        type: response
        config:
          output:
            summary: "gen.summary"
    edges:
      - from: gen
        to: result
```

---

### `skill_set` — Skill Definitions for Progressive Disclosure

Holds a collection of skill definitions that `generate` nodes can reference for progressive disclosure via tool calling.

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
          3. Prefer Result<T, E> over panicking
      - name: api_design
        description: "REST API design guidelines"
        content_file: "skills/api_design.md"
        references:
          - "docs/openapi_spec.md"
```

**Config fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `skills` | Yes | Array of skill definitions |
| `skills[].name` | Yes | Unique skill name (used in `load_skill` calls) |
| `skills[].description` | Yes | Short description shown in L1 metadata |
| `skills[].content` | No* | Inline skill content (L2 full text) |
| `skills[].content_file` | No* | External file path, resolved to content at execution |
| `skills[].references` | No | Array of file paths for L3 reference loading |

*One of `content` or `content_file` is required per skill.

**Output fields:**
- `{node_id}.skills` — Array of resolved skill objects (with `content_file` resolved to `content`)
- `{node_id}._skill_count` — Number of skills defined

**Progressive disclosure levels:**
1. **L1 — Metadata** (always in system_prompt): skill name + description list
2. **L2 — Full content** (on demand): LLM calls `load_skill("skill_name")` → gets full content
3. **L3 — References** (on demand): LLM calls `load_skill_reference("skill_name", "path")` → gets referenced file

**Connection to generate nodes:** A `skill_set` node must be connected via edges to any `generate` node that references it. The generate node also declares the reference in its `skill_sets` config field:

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
      skill_sets: ["coding_skills"]      # Must match skill_set node IDs
      # ... other config
```

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

  # Error edge (taken when source node fails with on_exhausted: continue)
  - from: risky_api
    to: fallback_handler
    on_error: true                       # Only activates when source has _error output
```

**Error routing**: When a node has `error_handling.on_exhausted: continue` and all retries fail,
the node produces `_error: true` and `_error_message: "..."` in its outputs.
- `on_error: true` edges activate only when the source node failed
- Normal edges from the same source are **suppressed** when `on_error` edges are active
- This enables fallback patterns without workflow termination

```yaml
# Example: risky call with fallback
nodes:
  - id: primary
    type: generate
    config:
      provider: openai
      model: gpt-4o
      template: my_prompt
      error_handling:
        retry: 2
        timeout_ms: 15000
        on_exhausted: continue

  - id: fallback
    type: generate
    config:
      provider: qwen
      model: qwen-plus
      template: fallback_prompt
      variables:
        error_info: "primary._error_message"

edges:
  - from: primary
    to: success_path          # taken when primary succeeds
  - from: primary
    to: fallback              # taken when primary fails after retries
    on_error: true
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
| `temperature` | float | 0.7 | Sampling temperature (clamped to provider/model max) |
| `max_tokens` | int | 2048 | Maximum tokens to generate |
| `top_p` | float | null | Top-p nucleus sampling (masked if provider doesn't support) |
| `top_k` | int | null | Top-k sampling (masked if provider doesn't support) |
| `thinking_budget` | int | null | Extended thinking token budget (masked if model.thinking = "none") |
| `reasoning_effort` | string | null | OpenAI o-series reasoning: "low", "medium", "high" |
| `enable_search` | bool | false | Enable web search (Qwen/DashScope only) |

All sampling parameters are subject to **capability masking** — the engine automatically removes
parameters that the selected provider/model doesn't support. See the Capability Masking table
in the `generate` node section.

### Output Schema Types

`string`, `number`, `boolean`, `string[]`, `number[]`, `object`, `object[]`

**Nested schemas** using `children`:

```yaml
output_schema:
  - name: analysis
    type: object
    children:
      - name: summary
        type: string
      - name: keywords
        type: string[]
      - name: metrics
        type: object
        children:
          - name: score
            type: number
          - name: confidence
            type: number
```

The engine generates example JSON recursively from nested schemas and appends it to the prompt.

**JSON extraction**: When `output_schema` is present, the engine:
1. Sends `response_format: json_object` to the provider (if model supports it)
2. Appends "Respond in JSON format: {example}" to the user prompt
3. Parses the response with robust extraction (handles markdown fences, prose wrapping, brace matching)
4. Maps schema fields to node outputs with type-appropriate defaults for missing fields

Templates can also be loaded from external files via `Engine::builder().prompts_dir("./prompts")`.
File-based templates are referenced by filename (without extension).

---

## Subgraphs

Subgraphs are mini-workflows used as tools by supervisor, iterator, and generate (tool calling) nodes.

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

## Dead Path Detection

When a conditional edge evaluates to false, the engine propagates **dead path** information
through the graph. A node is marked dead when **all** its incoming edges are inactive:

- The source node is itself dead (will never execute), **or**
- The edge is a conditional edge whose condition evaluated to false

Dead path detection runs after each batch of node completions and propagates transitively.

**Impact on execution:**
- Dead nodes are removed from the pending set (they never execute)
- Variables with `default` that reference dead nodes use the default immediately
- Variables **without** `default` that reference dead nodes cause a clear error
- No dummy/skip nodes are needed to handle conditional branch convergence

```
┌──────────┐  condition=true   ┌───────────┐
│  check   │─────────────────→│  process  │──┐
└──────────┘                   └───────────┘  │   ┌──────────┐
      │                                       ├──→│  merge   │
      │ condition=false                       │   └──────────┘
      │   ┌───────────┐                       │
      └──→│   skip    │ ← DEAD (not taken) ──┘
          └───────────┘

merge.result:
  from: process.output    ← waits (process is alive)
  default: ""             ← fires instantly if process is dead
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
