# rp-engine 完全代码调研报告

> 调研日期: 2026-03-30
> 覆盖范围: 全部 65 个源文件 (不含 target/)

## 一、项目概览

**名称**: `riceprompt-engine` v0.1.0
**语言**: Rust 2021 edition, 双许可 MIT/Apache-2.0
**定位**: YAML 原生的 Agent 工作流执行引擎 -- 一个声明式的 DAG (有向无环图) 工作流编排系统，用于编排 LLM 调用、数据连接、搜索、脚本变换等操作。

---

## 二、源文件清单 (共 65 个源文件，不含 target/)

| 目录 | 文件数 | 职责 |
|------|--------|------|
| `src/` (根) | 2 | lib.rs (crate入口), error.rs (统一错误) |
| `src/engine/` | 6 | 执行引擎核心：图构建、调度、执行、流式处理 |
| `src/schema/` | 5 | YAML 工作流/模板/节点/边的数据结构定义 |
| `src/state/` | 2 | 运行时状态机 (append-only 执行记录) |
| `src/expression/` | 4 | 边条件表达式语言 (词法/语法/求值) |
| `src/nodes/` | 11 | 10种内置节点执行器 + 自定义节点接口 |
| `src/nodes/connectors/` | 5 | 数据库/存储连接器 (PostgreSQL, Redis, Qdrant, S3) |
| `src/providers/` | 16 | LLM/Embedding/Search 提供商抽象 |
| `src/providers/search/` | 5 | 搜索提供商 (Tavily, Brave, Bocha, Perplexity) |
| `tests/` | 3+5 fixtures | 集成测试 + YAML 工作流 fixture |
| `examples/` | 3 | 示例程序 |
| `docs/` | 4 | 设计文档和审计报告 |

---

## 三、架构分层详解

### 3.1 错误体系 (`src/error.rs`)

四层错误枚举，全部基于 `thiserror`:

| 错误类型 | 变体数 | 用途 |
|----------|--------|------|
| `EngineError` | 36 | 主错误，覆盖工作流/执行/模板/Provider/状态/Supervisor/数据连接/Transform/搜索/状态保存/序列化 |
| `ProviderError` | 6 | LLM API 调用失败: ApiError, RateLimited, AuthenticationFailed, InvalidResponse, RequestFailed, Timeout |
| `ExprError` | 11 | 表达式解析/求值失败 |
| `ValidationError` | 6 | YAML 工作流结构校验 |

自动类型转换链: `ProviderError` -> `EngineError`, `ExprError` -> `EngineError`, `reqwest::Error` -> `EngineError`, `serde_yaml/json::Error` -> `EngineError`

---

### 3.2 Schema 层 (`src/schema/`)

定义工作流的静态数据结构，对应 YAML 文件。

#### 3.2.1 `PreparedWorkflow` (workflow.rs)
工作流顶层容器:
- `version`: 支持 `"0.1"` 和 `"1.0"`
- `name`, `description`: 标识信息
- `input: InputSchema`: 输入参数定义 (name/type/required/default)，带 `validate()` 方法
- `data_sources: HashMap<String, DataSourceDef>`: 数据源定义 (kind: rest_api/graphql/postgresql/redis/s3/qdrant)
- `nodes: Vec<YamlNode>`, `edges: Vec<YamlEdge>`: DAG 结构
- `templates: HashMap<String, Template>`: 附带的 Prompt 模板
- `transforms: HashMap<String, String>`: 内联 Rhai 脚本
- `subgraphs: HashMap<String, Subgraph>`: Supervisor 子图 (工具箱)

**验证逻辑** (`validate()`):
1. 版本号检查
2. 必须有且仅有一个 Start 节点
3. 至少一个 Response 节点
4. 节点 ID 不可重复
5. 边的 from/to 必须引用存在的节点
6. Generate 节点必须有 provider 和 model
7. 循环不在此检测 -- 依赖 edge 上的 `max_iterations` 保护

#### 3.2.2 `YamlNode` + `NodeConfig` (node.rs)

**`NodeType` 枚举** (10 种内置 + 1 种自定义):

| 类型 | 说明 |
|------|------|
| `Start` | 入口，映射输入字段 |
| `Generate` | LLM 调用 (支持流式/多模态/条件模板/结构化输出) |
| `Execute` | 代码执行 (未实现) |
| `Supervisor` | 计划-执行循环 (Agent 模式) |
| `DataConnector` | 外部数据源操作 |
| `Mcp` | MCP 协议工具 (未实现) |
| `Response` | 终端输出 |
| `Checkpoint` | 暂停/恢复点 |
| `Transform` | Rhai 脚本计算 |
| `Search` | 网络搜索 |
| `Custom(String)` | 用户自定义 (catch-all) |

**`NodeConfig`** 是一个大型扁平结构体，所有节点类型共享，通过 `Option` 区分:
- **通用**: `output`, `variables`
- **Generate**: `template` (支持条件选择), `provider`, `model`, `stream`, `images`
- **Supervisor**: `tools`, `max_iterations`, `parallel`, `timeout`, `max_result_length`, `summarizer_template`
- **DataConnector**: `data_source_id`, `operation`, 或遗留 `source`+`connection`
- **Transform**: `file` 或 `script` (inline Rhai)
- **Workflow**: `workflow` (子工作流名)

**`VariableBinding`** (3 种形式):
- `Single("path.to.field")` -- 简单路径引用
- `Multiple(["path1", "path2"])` -- 多源优先取值
- `Configured { from, default }` -- 带默认值的配置

**`TemplateSelector`** (2 种形式):
- `Simple("template_name")` -- 直接引用
- `Conditional { default, conditions: [{when, use}] }` -- 条件选择 (第一个匹配生效)

两者都有手写的 `Deserialize` 实现，以支持 YAML 中的字符串/对象双语法。

#### 3.2.3 `YamlEdge` (edge.rs)

边定义: `from`, `to`, 加上:
- `condition: Option<String>` -- 布尔表达式，满足才走
- `max_iterations: Option<u32>` -- 循环保护上限
- `is_default: bool` -- 无条件/回退边
- `exit_reason: Option<ExitReason>` -- Supervisor 退出路由

**`ExitReason`** 枚举: `TodoEmpty`, `Timeout`, `MaxIterations`, `MaxResultLength`, `ToolFailures`, `Default`

#### 3.2.4 `Template` (template.rs)

Prompt 模板:
- `user_prompt`: 用户提示 (Handlebars `{{variable}}` 模板)
- `system_prompt`: 可选系统提示
- `config: TemplateConfig`: temperature (默认0.7), max_tokens (默认2048), top_p, top_k, thinking_budget, enable_search
- `output_schema: Vec<OutputField>`: 结构化输出 (name/type/description/example)

---

### 3.3 状态层 (`src/state/`)

#### `ExecutionState` -- 核心运行时状态

**设计哲学: Append-Only (只追加)**

每次节点执行产生一条 `ExecutionRecord`:
- `short_id`: base36 唯一标识 (a, b, ..., z, 0, ..., 9, ba, bb, ...)
- `execution_index`: 0-based 执行序号
- `outputs: HashMap<String, Value>`: 输出数据
- `timestamp`: 毫秒时间戳

**主要数据结构:**
- `node_executions: HashMap<String, Vec<ExecutionRecord>>` -- 每节点的执行历史
- `latest_execution: HashMap<String, String>` -- 缓存的最新 short_id
- `input: HashMap<String, Value>` -- 工作流输入
- `edge_counts: HashMap<(String,String), u32>` -- 边遍历计数 (循环保护)
- `supervisor_context: Option<SupervisorContext>` -- Supervisor 运行时上下文
- `dead_nodes/dead_edges` -- 死路径检测

**路径表达式系统** (核心创新):

| 路径格式 | 含义 |
|----------|------|
| `node.field` | 最新执行的 field 值 |
| `node[0].field` | 第一次执行 |
| `node[-1].field` | 最后一次执行 |
| `node[abc].field` | 指定 short_id 的执行 |
| `node[*].field` | 所有执行的 field 组成数组 |
| `input.field` | 工作流输入 |

**Checkpoint 机制:**
- `to_checkpoint()`: 序列化完整状态 -> `CheckpointState`
- `from_checkpoint()`: 从检查点恢复，合并新输入

---

### 3.4 表达式引擎 (`src/expression/`)

经典的 **Lexer -> Parser -> Evaluator** 三阶段管道:

#### 词法分析器 (lexer.rs)
- 支持: 字符串(单/双引号+转义), 数字(f64), 布尔, null, 标识符
- 运算符: `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `||`, `!`
- 分隔符: `(`, `)`, `[`, `]`, `.`, `,`

#### 语法分析器 (parser.rs)
递归下降解析，按优先级: `||` < `&&` < `!` < 比较 < 原子值

**AST 节点** (`Expr` 枚举):
- `Or`, `And`, `Not` -- 逻辑运算
- `Compare(lhs, op, rhs)` -- 比较
- `Path(PathExpr)` -- 路径引用 (truthiness 检查)
- `Literal(Value)` -- 字面量
- `FunctionCall(name, arg)` -- 内置函数

**内置函数**: `empty(expr)`, `exists(expr)`, `len(expr)`

#### 求值器 (evaluator.rs)
- `evaluate_expression(expr_str, state) -> Result<bool>`: 唯一入口
- 路径解析委托给 `ExecutionState::resolve_path()`
- truthiness 规则: null->false, 空字符串/数组/对象->false, 0->false
- 支持跨类型比较: `"true" == true` 成立

---

### 3.5 执行引擎 (`src/engine/`)

#### 3.5.1 `ExecutionGraph` (graph.rs)

零拷贝图表示，借用 `PreparedWorkflow` 的引用:
- `node_map: HashMap<&str, &YamlNode>` -- 快速节点查找
- `outgoing_edges / incoming_edges` -- 邻接表

关键方法:
- **`inputs_satisfied(node_id, state)`** -- 核心依赖检查:
  1. Start 节点始终就绪
  2. 至少一条入边的源节点有输出
  3. 所有 `variables` 绑定可解析 (路径引用有值，或所有源已"定局"时可用默认值)
- **`topological_sort()`** -- DFS 拓扑排序 (检测循环)

#### 3.5.2 执行循环 (`executor.rs`)

**`execute_workflow()`** -- 核心函数:

1. **输入校验**: 验证必填字段
2. **构建图**: `ExecutionGraph::from_workflow()`
3. **初始化状态**: 全新或从 checkpoint 恢复
4. **主循环** (基于输入的依赖调度):
   - 检查超时 -> 找就绪节点 -> 并行执行 -> 记录结果 -> 评估边 -> 传播死路径
   - **就绪判定**: `graph.inputs_satisfied()` + 未执行 + 未在飞行中
   - **并行执行**: `futures::future::join_all()` 并发执行所有就绪节点
   - **Checkpoint 处理**: 遇到 checkpoint 节点立即返回 `Paused` 状态
   - **边评估**: 条件表达式求值 -> 确定下一批节点
   - **死路径传播**: 不动点算法，标记所有入边均不可达的节点为 dead

5. **恢复执行**: 从 checkpoint 恢复时，BFS 计算前向可达节点

**关键数据结构:**

**`ExecutionConfig`** -- 配置对象:
- Engine 级: providers, embedding_providers, search_providers, node_registry, pools, workflows, model_registry, transforms
- Workflow 级: templates, subgraphs, data_sources
- Run 级: timeout, max_iterations, capture_actual_prompt, capture_raw_response, stream_handler

**`ExecutionResult`** -- 返回值:
- `status`: Success / Error / Timeout / Paused
- `executions`: 完整执行历史
- `output`: 最终输出
- `timing / token_usage`: 性能和消耗追踪
- `checkpoint_state`: Paused 时存在

#### 3.5.3 `Engine` (engine.rs) -- 用户面 API

核心字段: workflows, templates, providers (LLM/embedding/search), node_registry, pools, model_registry, transforms, state_saving, timeout (默认 300s), max_iterations (默认 100)

公开方法:
- `Engine::builder()` -> `EngineBuilder` -> `Engine`
- `run(workflow_name, input)` -- 执行工作流
- `run_with_stream_handler(...)` -- 带流式处理
- `resume(...)` -- 从 checkpoint 恢复
- `run_yaml(yaml, input)` -- 动态 YAML 执行

#### 3.5.4 `EngineBuilder` (builder.rs)

流畅 API:
```rust
Engine::builder()
    .workflows_dir("./workflows")
    .prompts_dir("./prompts")
    .transforms_dir("./transforms")
    .provider("openai", "sk-xxx")
    .embedding_provider("dashscope", "sk-xxx")
    .search_provider("tavily", "tvly-xxx")
    .state_saving(true, "./state")
    .build().await?
```

**`build()` 关键步骤:**
1. 注册内置 `WorkflowNode` + 用户自定义节点
2. 创建 LLM/Search/Embedding Provider Registry
3. 加载 workflows (`.yaml`), templates, transforms (`.rhai`)
4. 创建 Redis 连接池
5. **Vision 校验**: 遍历所有 Generate 节点，若配置了 `images` 则验证 provider/model 支持视觉
6. 环境变量展开: `${VAR_NAME}` -> 环境变量值

#### 3.5.5 `StreamHandler` (stream_handler.rs)

```rust
pub trait StreamHandler: Send + Sync {
    fn on_text_chunk(&self, node_id: &str, chunk: &str, accumulated: &str);
}
pub type SharedStreamHandler = Arc<dyn StreamHandler>;
```
同步接口，异步操作应在实现内用 `tokio::spawn`。

---

### 3.6 节点执行器 (`src/nodes/`)

#### 通用机制

**`NodeExecutor` trait**: `async fn execute(node, state, config) -> Result<HashMap<String,Value>>`

**`CustomNode` trait**: 用户扩展接口，通过 `NodeRegistry` 注册

**`create_executor()` 工厂函数**: NodeType -> 具体 Executor

**共享变量解析** `resolve_variables()`:
- 包含 `.` 的值视为状态路径，从 `ExecutionState` 读取
- 不含 `.` 的值视为字面量 (`true`/`false`/数字/字符串)
- 最终 fallback 到 binding 的 default_value

#### 各节点详解

| 节点 | 执行器 | 核心逻辑 |
|------|--------|----------|
| **Start** | `StartExecutor` | 映射 `config.output` 或直接传递所有输入 |
| **Generate** | `GenerateExecutor` | LLM 调用: 条件模板选择 -> Handlebars 渲染 -> 多模态构建 -> Provider chat/stream -> JSON 解析 |
| **Response** | `ResponseExecutor` | 从状态映射字段到输出 |
| **Checkpoint** | `CheckpointExecutor` | 同 Response，但触发引擎暂停 |
| **Transform** | `TransformExecutor` | 创建沙箱 Rhai 引擎(限制10万操作/1MB字符串/1万元素)，注入变量，执行脚本 |
| **Search** | `SearchExecutor` | 选择搜索 Provider -> 构建 SearchOptions -> 调用 search() -> 返回 results/answer |
| **Supervisor** | `SupervisorExecutor` | **最复杂**: Planner LLM -> TodoList -> 依赖感知执行 -> 子图执行 -> 退出条件检查 |
| **DataConnector** | `DataConnectorExecutor` | 路由到具体连接器 (REST/GraphQL/S3/PG/Qdrant/Redis) |
| **Workflow** | `WorkflowNode` (CustomNode) | 子工作流执行: 变量映射 -> execute_child_workflow -> 结果回传 |
| **Execute/Mcp** | `NotImplementedExecutor` | 占位，返回错误 |

#### Generate 节点细节:
- **条件模板**: 按顺序评估 `conditions[].when`，第一个 true 选中
- **多模态**: 支持 base64、data URI、HTTP URL 三种图片输入
- **结构化输出**: 生成 JSON 示例注入 prompt，解析响应映射到输出字段
- **流式**: `stream=true` + handler 存在 + 无 JSON schema 时启用

#### Supervisor 节点细节:
- **Planner**: 单次 LLM 调用生成 TodoList (action_id, action_called, action_prerequest, action_detail, action_layer)
- **依赖感知**: `action_prerequest` 决定执行顺序，支持并行
- **死锁处理**: 如果无就绪 todo 但有待处理，强制执行第一个
- **退出条件**: TodoEmpty, Timeout, MaxIterations, MaxResultLength, ToolFailures
- **子图执行**: 创建隔离 ExecutionState，按拓扑序执行子图节点

#### 数据连接器:

**PostgreSQL** (`connectors/postgresql.rs`):
- 5 种操作: `query`, `insert`, `select`, `update`, `delete`
- 参数化查询: `{paramName}` -> `$1, $2...` (防 SQL 注入)
- 连接池: `deadpool_postgres::Pool`
- `delete` 强制要求 WHERE 子句

**Redis** (`connectors/redis.rs`):
- 7 种操作: `get`, `set`, `del`, `publish`, `json_append`, `json_merge`, `json_delete`
- 原子 JSON 操作: 3 个嵌入式 Lua 脚本
- 类型保留: `{paramName}` 精确匹配时注入原始类型

**Qdrant** (`connectors/qdrant.rs`):
- 7 种操作: `search`, `scroll`, `get_points`, `upsert`, `batch_upsert`, `delete`, `create_collection`
- 自动嵌入: 从 text 字段调用 EmbeddingProvider

**S3** (`connectors/s3.rs`):
- 7 种操作: `get_object`, `put_object`, `list_objects`, `delete_object`, `presigned_url`, `head_object`, `copy_object`
- `force_path_style=true` (兼容 MinIO)

---

### 3.7 Provider 层 (`src/providers/`)

#### 3.7.1 核心抽象

**`LLMProvider` trait**:
```rust
async fn chat(messages, options) -> Result<ChatResponse, ProviderError>
async fn stream(messages, options, on_chunk) -> Result<StreamResponse, ProviderError>
```

**`EmbeddingProvider` trait**:
```rust
async fn embed(texts, options) -> Result<EmbeddingResponse, ProviderError>
```

**`SearchProvider` trait**:
```rust
async fn search(query, options) -> Result<SearchResponse, ProviderError>
```

#### 3.7.2 LLM Provider 架构 (三层)

| 层级 | Provider | API 特殊性 |
|------|----------|-----------|
| **定制实现** | OpenAI | reasoning model 检测 (o系列/GPT-5), reasoning_effort |
| | Anthropic | x-api-key 头, 独立 system 字段, 扩展思考 beta, 结构化输出 beta |
| | Gemini | API key 在 URL query, 思考内容分离, thinkingConfig |
| **共享基底** | OpenAICompatibleProvider | 通用 OpenAI 兼容实现, Qwen enable_search 特殊处理 |
| **委托包装** | DeepSeek, xAI, Qwen, Zhipu, Moonshot, MiniMax, Huoshan | ~63 行，全部委托 |

#### 3.7.3 Embedding & Search Provider

| 类型 | Provider | 默认模型 |
|------|----------|----------|
| Embedding | OpenAI | text-embedding-3-large |
| Embedding | DashScope | text-embedding-v4 |
| Search | Tavily | - |
| Search | Brave | - |
| Search | Perplexity | sonar |
| Search | Bocha | - |

#### 3.7.4 Model Registry (`model_registry.rs`)

从编译时嵌入的 `providers.json` 构建 vision 模型集合:
- 8 个 provider，37 个模型
- `supports_vision(provider, model)` 在 build 时校验

---

### 3.8 Rhai 脚本 (Transform 节点)

沙箱引擎:
- **限制**: 10万操作, 1MB 字符串, 1万数组/map 元素
- **内置函数**: `now()` (北京时间), `uuid()` (v4), `json_encode(value)`, `unique(array)`

---

## 四、依赖关系

| 类别 | 依赖 | 用途 |
|------|------|------|
| 异步运行时 | tokio (full), async-trait, futures | 全异步执行 |
| 序列化 | serde + derive, serde_json, serde_yaml | YAML/JSON 处理 |
| HTTP | reqwest (json+stream) | Provider API 调用 |
| 对象存储 | aws-sdk-s3, aws-config | S3/MinIO 连接 |
| PostgreSQL | tokio-postgres, deadpool-postgres | 连接池 |
| 向量数据库 | qdrant-client | Qdrant 操作 |
| Redis | redis (tokio-comp, json, connection-manager) | 异步 Redis |
| 模板 | handlebars v6 | Prompt 渲染 |
| 脚本 | rhai (sync) | Transform 节点 |
| 错误 | thiserror | 错误派生 |
| 日志 | tracing | 结构化日志 |
| 工具 | regex-lite, base64, chrono, uuid | 辅助功能 |

---

## 五、YAML Provider 注入机制 (2026-03-30 新增)

### 5.1 背景与动机

此前 LLM API key 只能通过 `EngineBuilder::provider("openai", key)` 在 Rust 代码层注入。YAML 中的 Generate 节点只写 provider 名称（如 `provider: openai`），无法携带 API key。这导致：
1. Server 共享 Engine 单例无法支持 per-user / per-workflow 的 API key
2. 工作流无法自包含地声明所需的 provider 配置

### 5.2 凭据注入路径总览

| 凭据类型 | YAML 可注入？ | 注入方式 |
|----------|-------------|---------|
| LLM API Key | **可以** (新增) | YAML `providers` 字段 + `${ENV_VAR}` 展开，或 `EngineBuilder::provider()` |
| Embedding API Key | 不能 | 仅 `EngineBuilder::embedding_provider()` |
| Search API Key | 不能 | 仅 `EngineBuilder::search_provider()` |
| PostgreSQL 密码 | 可以 | YAML `data_sources.config` + `${ENV_VAR}` |
| Redis 密码 | 可以 | YAML `data_sources.config` 或 `EngineBuilder::redis()` |
| S3 access_key/secret_key | 可以 | YAML `data_sources.config` |
| Qdrant API Key | 可以 | YAML `data_sources.config` |
| REST/GraphQL auth | 可以 | YAML `data_sources.config.auth` |

### 5.3 新增 `ProviderDef` 结构体

**文件**: `src/schema/workflow.rs`

```rust
pub struct ProviderDef {
    pub kind: Option<String>,        // 省略时默认用 map key
    pub api_key: String,             // 支持 ${ENV_VAR} 展开
    pub base_url: Option<String>,    // 自定义端点 (openai/anthropic/openai_compatible)
    pub default_model: Option<String>, // 仅 openai_compatible
}
```

- 自定义 `Debug` 实现遮蔽 `api_key`（仅显示末 4 位），防止日志泄露
- `kind` 支持: `openai`, `anthropic`, `gemini`, `deepseek`, `xai`, `qwen`, `zhipu`, `moonshot`, `minimax`, `huoshan`, `openai_compatible`

### 5.4 `PreparedWorkflow.providers` 字段

```rust
#[serde(default)]
pub providers: HashMap<String, ProviderDef>,
```

`#[serde(default)]` 确保无该字段的旧 YAML 正常解析（空 HashMap），完全向后兼容。

### 5.5 Provider 工厂函数

**文件**: `src/providers/mod.rs`

```rust
pub fn create_provider_from_def(
    provider_id: &str,
    def: &ProviderDef,
) -> Result<Arc<dyn LLMProvider>, EngineError>
```

内部按 `kind`（或 fallback 到 `provider_id`）match 到对应构造器。`openai_compatible` 要求必填 `base_url`。

### 5.6 合并逻辑

在三个执行入口点（`run_internal()`, `resume()`, `run_yaml()`），紧跟 `data_sources` 合并之后，遍历 `workflow.providers` 创建 provider 实例并注册到 `config.providers`。YAML 级 provider **覆盖**同名 engine 级 provider。

同时修复了 `run_yaml()` 缺失的 `data_sources` 合并（pre-existing bug）。

### 5.7 YAML 语法

```yaml
version: "1.0"
name: my_workflow

providers:
  openai:
    api_key: "${OPENAI_API_KEY}"          # 环境变量引用
  anthropic:
    api_key: "${ANTHROPIC_API_KEY}"
    base_url: "https://proxy.example.com" # 可选自定义端点
  my_local_llm:
    kind: openai_compatible               # 自定义 OpenAI 兼容端点
    api_key: "not-needed"
    base_url: "http://localhost:8000/v1/chat/completions"
    default_model: "llama-3-8b"

nodes:
  - id: gen
    type: generate
    config:
      provider: my_local_llm              # 引用 YAML 定义的 provider
      model: llama-3-8b
```

### 5.8 Server 集成方式

1. **Server 全局 key**: 继续用 `EngineBuilder::provider()` 在启动时注入
2. **Per-user key via YAML**: Server 在调用 `run_yaml()` 前将用户 key 注入 YAML 的 `providers` 段
3. **编程注入**: 导出的 `create_provider_from_def()` 允许 Server 自行构建 `ProviderRegistry` 用于未来的 `run_with_providers()` API

### 5.9 修改文件清单

| 文件 | 变更 |
|------|------|
| `src/schema/workflow.rs` | +`ProviderDef` 结构体, +`PreparedWorkflow.providers` 字段, +3 测试 |
| `src/providers/mod.rs` | +`create_provider_from_def()` 工厂函数, +6 测试 |
| `src/engine/engine.rs` | 3 处 provider 合并 + 修复 `run_yaml()` data_sources 合并 |
| `src/schema/mod.rs` | Re-export `ProviderDef` |
| `src/lib.rs` | Re-export `ProviderDef`, `create_provider_from_def` |

---

## 六、已知架构问题（部分已修复）

1. **双 API 路径**: `Engine.run()` vs 独立 `execute_workflow()` -- 建议统一
2. **ExecutionConfig 过大**: "上帝对象" -- 建议拆分
3. **data_sources 重复**: 每个 YAML 都要声明 -- 建议两层注册
4. **双变量绑定语法**: 新旧共存 -- 建议统一
5. **表达式语言不足**: 缺少算术、字符串操作、类型转换
6. **Anthropic 流式 usage bug**: input_tokens 在 streaming 中丢失
7. **硬编码 API 密钥**: 测试/示例文件中存在明文密钥

---

## 七、发展路线

| 阶段 | 状态 | 内容 |
|------|------|------|
| Phase 1: 规范化 | 已完成 | FLOW_SPEC.md, 变量绑定统一 |
| Phase 2: 节点增强 | 进行中 | Transform 节点, 增强 connectors |
| Phase 3: Agent 增强 | 计划中 | MCP 集成, Supervisor 增强 |
| Phase 4: IDE 集成 | 计划中 | IDE-ENGINE-ALIGNMENT-GUIDE |
