# MCP 节点设计讨论记录

> 状态：设计讨论中，未开始实现

---

## 一、已确认

### 1. 传输协议
- **仅 Streamable HTTP**（不做 stdio）
- 适合云端部署场景，无需管理子进程

### 2. 两种使用场景

#### 场景 A：显式调用（MCP 节点）
Workflow 设计者明确指定调用哪个 MCP Server 的哪个 Tool，一进一出，类似 data_connector。

```yaml
- id: search
  type: mcp
  config:
    server: my_mcp_server        # Engine 或 workflow 级注册的 MCP server ID
    tool: "web_search"           # 指定 tool 名称
    arguments:                   # 参数来自 variables
      query: start.user_query
    variables:
      query: start.user_query
```

输出：tool 返回的结果，映射为节点 output fields。

#### 场景 B：LLM 自主调用（Tool Use 循环）
LLM 通过 function calling 自主决定调用哪些 MCP tools，Engine 作为中间人执行往返循环。

```
Engine                    LLM Provider              MCP Server
  │── messages + tools ──→│                           │
  │←── tool_use(name,args)│                           │
  │── call_tool(name,args)───────────────────────→    │
  │←── tool_result ──────────────────────────────     │
  │── messages + result ─→│                           │
  │←── final answer ──────│  （或继续调下一个 tool）    │
```

关键点：
- LLM 不能直接调用 MCP Server，**Engine 是中间人**
- LLM 返回 tool_use 消息（tool 名 + 参数），Engine 代为执行
- 可能多轮往返（LLM 调完一个 tool 又要调另一个）
- 本质上是一个 **mini agent loop**

---

## 二、待确认

### 1. 场景 B 的节点形式

三种方案，未选定：

| 方案 | 说明 | 优劣 |
|------|------|------|
| **generate 增强** | 在 generate 节点的 config 中增加 `mcp_tools` 字段 | 简单直接，但 generate 职责膨胀 |
| **独立 mcp_agent 节点** | 新节点类型，专门处理 LLM + MCP tool 循环 | 职责清晰，但与 supervisor 有重叠 |
| **复用 supervisor** | 让 supervisor 的 tools 支持 `mcp_server` 引用（而非仅 subgraph） | 复用现有能力，但 supervisor 已经很复杂 |

```yaml
# 方案 1: generate 增强
- id: chat
  type: generate
  config:
    provider: openai
    model: gpt-4o
    template: chat_tpl
    mcp_tools:
      - server: my_mcp
        tools: ["search", "calc"]
    max_tool_rounds: 5
    variables:
      message: start.input

# 方案 2: 独立 mcp_agent 节点
- id: agent
  type: mcp_agent
  config:
    provider: openai
    model: gpt-4o
    template: agent_tpl
    mcp_servers:
      - my_mcp
      - another_mcp
    max_rounds: 10
    variables:
      message: start.input

# 方案 3: 复用 supervisor
- id: agent
  type: supervisor
  config:
    provider: openai
    model: gpt-4o
    template: planner_tpl
    tools:
      - name: search
        description: "Search the web"
        mcp_server: my_mcp      # NEW: 引用 MCP server
        mcp_tool: search        # 而非 ref subgraph
      - name: db_query
        ref: db_subgraph        # 现有 subgraph 方式
```

### 2. MCP 能力范围

MCP 协议定义了 4 种能力：

| 能力 | 说明 | 是否第一版支持？ |
|------|------|----------------|
| **Tools** | 函数调用（搜索、计算、API 操作） | ？（大概率是） |
| **Resources** | 提供数据（文件、DB 记录、配置） | ？ |
| **Prompts** | 提供 prompt 模板 | ？ |
| **Sampling** | 反向请求 client 做 LLM 调用 | ？（大概率否） |

### 3. MCP 连接管理

三种方案，未选定：

| 方案 | 说明 |
|------|------|
| **Engine 级长连接池** | Engine 启动时连接所有 MCP servers，类似 DB pool |
| **按需连接** | 节点执行时才连接，执行完断开 |
| **混合模式** | Engine 级可选预连接 + workflow 级按需连接（同 data_sources 模式） |

### 4. MCP Server 注册方式

```yaml
# 方案 A: Engine 级（代码注册）
Engine::builder()
    .mcp_server("my_mcp", "https://mcp.example.com/sse")
    .build()

# 方案 B: Workflow YAML 内定义
mcp_servers:
  my_mcp:
    url: "https://mcp.example.com/sse"
    auth:
      type: bearer
      token: "${MCP_TOKEN}"

# 方案 C: 两层都支持（同 data_sources 模式）
```

---

## 三、技术要点

### Streamable HTTP 传输

MCP 的 Streamable HTTP 传输基于：
- **POST** `/mcp` 发送 JSON-RPC 请求
- **SSE** 接收流式响应
- 支持 session 管理（`Mcp-Session-Id` header）

### 与现有系统的关系

| 现有模块 | MCP 如何关联 |
|----------|-------------|
| **ProviderRegistry** | 场景 B 需要 provider 支持 function calling（tools 参数） |
| **Template** | 场景 B 的 system prompt 需要描述可用 tools |
| **ExecutionConfig** | 需要新增 MCP server registry |
| **ConnectionPools** | 可能需要扩展支持 MCP 连接 |
| **Supervisor** | 场景 B 与 supervisor 的 tool 执行循环逻辑高度相似 |

### Provider Function Calling 支持现状

当前 LLMProvider trait：
```rust
async fn chat(&self, messages: Vec<Message>, options: ChatOptions) -> Result<ChatResponse, ProviderError>;
```

ChatOptions 中 **尚未支持 tools/functions 参数**。场景 B 需要扩展：
- ChatOptions 增加 `tools: Vec<ToolDefinition>`
- ChatResponse 增加 `tool_calls: Vec<ToolCall>`
- 各 provider 实现 function calling 支持

---

## 四、实现优先级建议

```
Phase 1: MCP 基础设施
  ├─ MCP Client（Streamable HTTP 传输）
  ├─ Tool discovery（list_tools）
  ├─ Tool invocation（call_tool）
  └─ MCP Server 注册机制

Phase 2: MCP 节点（场景 A）
  ├─ 显式调用指定 tool
  ├─ Variables 映射到 tool arguments
  └─ Tool 结果映射到节点 output

Phase 3: Tool Use 循环（场景 B）
  ├─ Provider function calling 支持（ChatOptions 扩展）
  ├─ LLM ↔ MCP 往返循环
  └─ 选定节点形式（generate 增强 / mcp_agent / supervisor 扩展）
```
