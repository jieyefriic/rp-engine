# Generate 节点 Harness 架构设计

## 背景

当前 generate 节点是一个 monolithic 执行器，将 prompt render → payload render → request → parse 四阶段全部耦合。随着 skill、context management、harness instructions 等需求的出现，prompt render 阶段需要变得可扩展。

**设计决策：不拆成两个独立 workflow 节点**（因为 tool calling loop 需要在同一节点内闭环），而是在 generate 节点内部将 prompt render 建设为可扩展的 pipeline。

---

## 架构总览

```
generate 节点内部 pipeline:

┌─ Prompt Render Pipeline (可扩展) ──────────────────────────┐
│  ① harness instructions  — 固定注入 (.md 项目规则)         │
│  ② skill metadata        — 从 skill_set 节点注入 (name+desc)│
│  ③ context retrieval     — 规则检索注入 (向量/关键词)       │
│  ④ template + variables  — 现有 Handlebars 渲染            │
│  ⑤ output_schema         — JSON 格式指令追加               │
│  ⑥ images                — 多模态内容                      │
└────────────────────────────────────────────────────────────┘
                        ↓ messages[]
┌─ Payload Render + Request (现有) ─────────────────────────┐
│  capability masking → provider payload → HTTP              │
│  tool calling loop (load_skill/load_skill_reference + MCP tools + FC) │
│  parse → outputs                                           │
└────────────────────────────────────────────────────────────┘
                        ↓
┌─ Post-Execution — ✅ Writeback COMPLETE ──────────────────┐
│  if harness.writeback configured:                         │
│    LLM call (provider/model) ← current memory + output    │
│    → updated memory → write to memory_file                │
│    → truncate to memory_char_limit (default 2200 chars)   │
│  best-effort: failure ≠ workflow failure (warning only)    │
└────────────────────────────────────────────────────────────┘
```

---

## 三个新概念详细设计

### 1. Harness Instructions

**定位**：项目级固定规则，类似 Claude Code 的 `CLAUDE.md`。始终注入 system_prompt，不需要 LLM 判断。

**运行时机**：
- 执行前：固定注入到 system_prompt 最前面
- 执行后：根据执行结果自更新（更新记忆、修正规则）

**数据来源**：
```yaml
# workflow YAML 中声明
harness:
  instructions: |
    你是一个专业的数据分析助手。
    始终用中文回复。
    输出格式必须符合公司规范。
  memory_file: "harness_memory.json"    # 可选：持久化记忆
```
或引用外部文件：
```yaml
harness:
  files:
    - "rules/project_rules.md"
    - "rules/output_format.md"
```

**Prompt 注入方式**：
```
system_prompt = [
  harness_instructions,        ← 最高优先级，永远在最前
  skill_metadata_list,         ← skill 元数据
  template.system_prompt,      ← 模板的 system prompt
].join("\n\n---\n\n")
```

**Post-Execution Writeback — ✅ COMPLETE**：

当配置了 `harness.writeback` 时，workflow 执行完成后引擎发起一次专用 LLM 调用，审查执行结果并更新记忆文件。

```
执行完成后:
  if harness.writeback configured:
    1. 读取当前 memory_file 内容（frozen snapshot）
    2. 调用 writeback LLM (provider/model)，输入 = 当前 memory + workflow 最终输出
    3. LLM 产出更新后的 memory
    4. 截断到 memory_char_limit（默认 2200 字符，~800 tokens）
    5. 写回 memory_file
    6. 下次执行时更新后的 memory 注入 harness instructions
  best-effort: writeback 失败不影响 workflow 成功（仅 warning 日志）
```

**WritebackConfig**：
```rust
pub struct WritebackConfig {
    pub provider: String,                  // LLM provider for writeback
    pub model: String,                     // Model (can be cheap/fast)
    pub prompt: Option<String>,            // Custom prompt (sensible default if omitted)
    pub memory_char_limit: Option<usize>,  // Max chars, default 2200
}
```

**实现影响**：
- `NodeConfig` 新增 `harness` 字段
- `PreparedWorkflow` 新增 harness 配置
- prompt render 阶段在 template 渲染之前注入 harness 内容
- post-execution 阶段新增 harness update 逻辑

---

### 2. Skill — ✅ COMPLETE (Phase 2)

**定位**：可复用的能力片段，按需加载。LLM 自行决定是否使用。

**架构决策（2026-04-12 确定）**：
- **Skills 是图节点（`skill_set` 类型），不是 workflow 级数据** — 在 IDE 中可见、可拖拽、可组合
- **通过 tool calling 实现渐进加载，不是文件系统读取** — rp-engine 作为 library 在容器中运行，无本地文件系统
- **Generate 节点显式引用 skill_sets，不自动继承** — 明确数据流，避免隐式依赖
- **规则触发（trigger_keywords）推迟** — LLM 自触发已足够，规则触发增加复杂度
- **Harness memory writeback — ✅ COMPLETE** — WritebackConfig: LLM 驱动记忆更新 + char limit 截断，best-effort

**实现流程**：

```
skill_set 节点 → 解析 skills → 解析 content_file → 写入 state
                                                        ↓
generate 节点 ← 读取 skill_sets 引用 ← 从 state 读取 skill index
  ↓
  ① 构建 metadata section (name + description)
  ② 注入 system_prompt 中间层
  ③ 注册 load_skill + load_skill_reference 内置 tool
  ↓
tool calling loop:
  LLM 看到 metadata → 调用 load_skill → 获取完整内容 → 使用知识回答
                     → 调用 load_skill_reference → 获取 L3 引用资源
```

**加载机制 — 三级渐进式**：

```
Level 1: 元数据常驻 (始终在 system_prompt 中)
  ┌──────────────────────────────────────┐
  │ ## Available Skills                   │
  │ - rust_conventions: Rust coding ...   │
  │ - api_design: REST API design ...     │
  │ (每个 ~50-100 tokens, 列表常驻)       │
  └──────────────────────────────────────┘

Level 2: LLM 通过 tool call 加载完整 skill
  LLM 推理: "用户要审查 Rust 代码，我需要加载 rust_conventions"
  → tool_call: load_skill(name: "rust_conventions")
  → 返回 skill 完整内容作为 tool result
  → LLM 继续推理，已获得 skill 知识

Level 3: Skill 内部引用的资源按需加载
  Skill 声明 references: ["docs/openapi_spec.md"]
  → tool_call: load_skill_reference(skill_name: "api_design", path: "docs/openapi_spec.md")
  → 验证 path 在 skill 声明的 references 中
  → 返回引用文件内容
```

**Skill 定义格式（作为 skill_set 节点配置）**：
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
            ...
        - name: api_design
          description: "REST API design guidelines"
          content_file: "skills/api_design.md"
          references:
            - "docs/openapi_spec.md"
```

**命名冲突检查**：用户定义的 tools 不能使用 `load_skill` 或 `load_skill_reference` 作为名称，引擎在构建时拒绝。

**已实现的文件**：
- `src/schema/node.rs` — `SkillDef` 结构体，`skill_sets` 配置字段
- `src/nodes/skill_set.rs` — `SkillSetExecutor`，content_file 解析，state 写入
- `src/nodes/generate.rs` — skill index 构建，metadata 注入，`dispatch_tool_call` 提取，load_skill/load_skill_reference 内置 tool 处理
- `src/nodes/mod.rs` — executor 注册
- `src/schema/mod.rs` — 导出

---

### 3. Context Management

**定位**：管理 LLM 能看到的上下文——检索相关信息、压缩历史、控制 context window 使用。

**两条检索路径**：

```
路径 A — 规则检索 (prompt render 阶段)
  在 prompt 组装时自动执行，不需要 LLM 参与
  ┌─────────────────────────────────────┐
  │ 触发条件: workflow 配置中声明        │
  │ 检索方式:                            │
  │   - 向量搜索 (embedding → qdrant)   │
  │   - 关键词匹配                       │
  │   - 固定 query 模板                  │
  │ 结果: 注入 user_prompt 或 context 区 │
  └─────────────────────────────────────┘

路径 B — LLM 驱动检索 (payload/request 阶段)
  LLM 通过 tool call 自行决定搜什么
  ┌─────────────────────────────────────┐
  │ 已有机制:                            │
  │   - MCP 节点 (外部 MCP server)      │
  │   - data_connector 节点 (DB/API)    │
  │   - tool calling (子图执行)          │
  │ LLM 自行组合使用                     │
  └─────────────────────────────────────┘
```

**Context Window 管理**：

```
问题: context 越长，LLM 推理质量越差 (context rot)

策略:
  1. 优先级排序
     harness instructions > skill content > retrieved context > history
     当 window 满时，从低优先级开始截断

  2. 压缩 (compaction)
     对话历史超过阈值时，用轻量 LLM 总结压缩
     保留关键信息，释放 context 空间

  3. 滑动窗口
     只保留最近 N 轮对话
     更早的内容移入 memory (harness update)
```

**配置方式**：
```yaml
# generate 节点配置
context:
  # 路径 A: 规则检索
  retrieval:
    - source: "qdrant_knowledge_base"
      query_from: "start.user_query"      # 用哪个变量做检索
      top_k: 5
      inject_as: "context"                 # 注入到 prompt 的哪个位置

  # Context window 管理
  max_context_tokens: 8000                 # 软上限
  compaction:
    enabled: true
    strategy: "summarize"                  # summarize | truncate | sliding_window
    threshold: 6000                        # 超过此值触发压缩
```

**实现影响**：
- `NodeConfig` 新增 `context` 配置
- prompt render 阶段：执行规则检索 → 注入结果
- 路径 B 无需改动（已有 tool calling + search/data_connector）
- 新增 context window 管理模块（token 计数、压缩策略）
- 压缩需要额外 LLM 调用（轻量模型）

---

## 实施优先级

```
Phase 1: 修复现有 bug (前置条件) — ✅ COMPLETE (2026-04-09)
  - Anthropic streaming token 计数丢失
  - tool_result 消息格式 (Anthropic/Gemini)
  - tool call 参数解析静默吞错

Phase 2: Harness Instructions — ✅ COMPLETE (2026-04-09, writeback 2026-04-12)
  - workflow YAML 新增 harness section
  - prompt render 注入 harness 内容
  - memory_file 读取 + frozen snapshot 注入
  - post-execution writeback: WritebackConfig, LLM 驱动记忆更新, char limit 截断

Phase 3: Skill System — ✅ COMPLETE (2026-04-12)
  - skill_set 节点类型 (SkillDef + SkillSetExecutor)
  - skill 元数据注入 system_prompt 中间层
  - load_skill + load_skill_reference 内置 tool
  - generate 节点 skill_sets 配置字段
  - 命名冲突检查
  - 规则触发（trigger_keywords）推迟 — LLM 自触发已足够

Phase 4: MCP Integration — ✅ COMPLETE (2026-04-13)
  - mcp 节点: 显式 MCP tool call (Streamable HTTP, JSON-RPC 2.0)
  - mcp_tools 节点: 连接 MCP server, tools/list 发现工具
  - generate 节点 mcp_tool_sets: LLM 驱动 MCP tool calling
  - McpClient: SSE + plain JSON 响应处理, session 管理
  - 两种认证: bearer token, api_key
  - 已通过 DeepWiki 公共 MCP server 验证

Phase 5: Context Management — PLANNED
  - 规则检索 (复用现有 data_connector 能力)
  - context window token 计数
  - compaction 策略
```

---

## 与前端对齐

| 概念 | 前端 IDE 需要的 UI | 优先级 |
|------|---------------------|--------|
| Harness | generate 节点新增 "Harness" tab：.md 内容编辑器 | Phase 2 |
| Skill | skill_set 节点配置面板 + generate 节点 skill_sets 引用选择 | Phase 3 ✅ (engine done, IDE TBD) |
| Context | generate 节点新增 "Context" tab：检索源配置 + 压缩策略 | Phase 5 |
| Tool Calling | 修复现有 bug 后，generate 节点 tools section 正常工作 | Phase 1 |
| MCP | MCP server 连接管理 + mcp/mcp_tools 节点配置面板 + generate 节点 mcp_tool_sets 选择 | Phase 4 ✅ (engine done, IDE TBD) |
