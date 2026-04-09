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
│  ② skill metadata        — 固定注入 (name+desc 列表)       │
│  ③ context retrieval     — 规则检索注入 (向量/关键词)       │
│  ④ template + variables  — 现有 Handlebars 渲染            │
│  ⑤ output_schema         — JSON 格式指令追加               │
│  ⑥ images                — 多模态内容                      │
└────────────────────────────────────────────────────────────┘
                        ↓ messages[]
┌─ Payload Render + Request (现有) ─────────────────────────┐
│  capability masking → provider payload → HTTP              │
│  tool calling loop (含 skill 加载 tool、MCP tool、原生 FC) │
│  parse → outputs                                           │
└────────────────────────────────────────────────────────────┘
                        ↓
┌─ Post-Execution ──────────────────────────────────────────┐
│  harness update (记忆/规则自更新)                          │
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

**Post-Execution 自更新**：
```
执行完成后:
  if harness.memory_file:
    读取当前 memory
    根据本次执行结果追加/更新
    写回 memory_file
    下次执行时 memory 内容也注入 harness instructions
```

**实现影响**：
- `NodeConfig` 新增 `harness` 字段
- `PreparedWorkflow` 新增 harness 配置
- prompt render 阶段在 template 渲染之前注入 harness 内容
- post-execution 阶段新增 harness update 逻辑

---

### 2. Skill

**定位**：可复用的能力片段，按需加载。LLM 自行决定是否使用。

**加载机制 — 三级渐进式**：

```
Level 1: 元数据常驻 (始终在 system_prompt 中)
  ┌──────────────────────────────────────┐
  │ Available Skills:                     │
  │ - data_analysis: 数据分析和可视化建议 │
  │ - code_review: 代码审查和优化建议     │
  │ - sql_expert: SQL 查询生成和优化      │
  │ (每个 ~50-100 tokens, 列表常驻)       │
  └──────────────────────────────────────┘

Level 2: LLM 通过 tool call 加载完整 skill
  LLM 推理: "用户要做数据分析，我需要加载 data_analysis skill"
  → tool_call: load_skill("data_analysis")
  → 返回 skill 完整内容作为 tool result
  → LLM 继续推理，已获得 skill 知识

Level 3: Skill 内部引用的资源按需加载
  Skill 内容可能引用: schema.json, examples.md
  → LLM 再次通过 tool call 加载
```

**Skill 定义格式**：
```yaml
# skills/data_analysis.yaml
name: data_analysis
description: "数据分析和可视化建议，包含 pandas/matplotlib 最佳实践"
trigger_keywords: ["分析", "数据", "图表", "可视化"]   # 可选：规则触发
content: |
  ## Data Analysis Skill
  当进行数据分析时，遵循以下原则：
  1. 先理解数据结构和类型
  2. 检查缺失值和异常值
  3. 选择合适的可视化方式
  ...
references:                    # Level 3 按需加载
  - "schemas/data_types.json"
  - "examples/chart_patterns.md"
```

**两条触发路径**：

| 路径 | 机制 | 何时用 |
|------|------|--------|
| 规则触发 | `trigger_keywords` 匹配 input | 简单场景，零延迟 |
| LLM 自触发 | `load_skill` tool call | 复杂场景，LLM 自行判断 |

**不需要 checkpoint 的情况**（大多数）：
- Skill 内容作为 tool result 进入对话
- 在当前 tool calling loop 内完成
- 适合一次性任务

**需要 checkpoint 的情况**（少数）：
- Skill 需要注入到 system prompt 级别（改变 LLM 的"底层人格"）
- 需要跨多轮对话持久保持 skill
- 实现：checkpoint → 重新 prompt render（含 skill）→ resume

**实现影响**：
- workflow YAML 新增 `skills` section 定义 skill 列表
- prompt render 阶段注入 skill 元数据到 system_prompt
- 新增内置 tool `load_skill` 供 LLM 在 tool calling loop 中调用
- 可选：规则触发引擎（关键词匹配 → 自动预加载）

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
  │   - search 节点 (Tavily/Brave/...)  │
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
Phase 1: 修复现有 bug (前置条件)
  - Anthropic streaming token 计数丢失
  - tool_result 消息格式 (Anthropic/Gemini)
  - tool call 参数解析静默吞错

Phase 2: Harness Instructions (最简单，价值高)
  - workflow YAML 新增 harness section
  - prompt render 注入 harness 内容
  - post-execution harness memory 更新

Phase 3: Skill System
  - skill 定义格式 + 存储
  - skill 元数据注入 system_prompt
  - load_skill 内置 tool
  - 可选：规则触发引擎

Phase 4: Context Management
  - 规则检索 (复用现有 data_connector/search 能力)
  - context window token 计数
  - compaction 策略
```

---

## 与前端对齐

| 概念 | 前端 IDE 需要的 UI | 优先级 |
|------|---------------------|--------|
| Harness | generate 节点新增 "Harness" tab：.md 内容编辑器 | Phase 2 |
| Skill | workflow 级别 skill 管理面板 + generate 节点 skill 选择 | Phase 3 |
| Context | generate 节点新增 "Context" tab：检索源配置 + 压缩策略 | Phase 4 |
| Tool Calling | 修复现有 bug 后，generate 节点 tools section 正常工作 | Phase 1 |
| MCP | MCP server 连接管理 + tool 暴露给 generate 节点 | Phase 3+ |
