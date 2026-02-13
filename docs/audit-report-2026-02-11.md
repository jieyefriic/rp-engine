# rp-engine 全面审计报告

> 审计日期: 2026-02-11
> 审计范围: rp-engine + panda-ai-backend (实际使用方)

## 一、项目定位评估

目标：新一代声明式 Agent 开发框架。当前状态：**可用但不成熟的底板**。

**做对了的事情：**
- YAML 声明式 workflow，抽象层次选得准
- 基于输入依赖的节点执行（不是死板的 layer-based），正确的 DAG 执行模型
- Append-only 状态 + short-id 引用，可观测性好
- CustomNode trait 可扩展
- Supervisor 节点（Planning Loop）是真正的 agent 能力
- Checkpoint/Resume 支持暂停续跑
- 8 个 LLM Provider（OpenAI, Anthropic, Gemini, DeepSeek, Qwen, xAI, Zhipu, Moonshot + OpenAI-compatible）

**核心问题：** 没有规范。panda-ai-backend 里 13 个 custom node 说明引擎能力缺口大。

---

## 二、架构层面的关键问题

### 问题 1：双 API 路径 — 必须统一

```
路径 A: Engine::builder().build() → engine.run(name, input)    // panda-ai-backend 用的
路径 B: execute(yaml_str, input, ExecutionConfig)               // lib.rs 示例用的
```

**为什么这是问题：**
- `execute()` 是无状态独立函数，需要调用方自己拼 `ExecutionConfig`
- `Engine.run()` 内部也是构造 `ExecutionConfig` 然后调 `execute_workflow()`
- 两条路径共享 `execute_workflow()` 底层，但暴露的心智模型完全不同
- IDE/Server 用 `execute()` 需要了解引擎内部结构来拼 config

**建议：** 废弃公开的 `execute()` 函数。统一为 `Engine` 作为唯一入口。

### 问题 2：ExecutionConfig 是"上帝对象"

```rust
pub struct ExecutionConfig {
    pub providers: ProviderRegistry,
    pub embedding_providers: EmbeddingRegistry,
    pub timeout: Duration,
    pub max_iterations: u32,
    pub templates: HashMap<String, Template>,
    pub subgraphs: HashMap<String, Subgraph>,
    pub data_sources: HashMap<String, DataSourceDef>,
    pub node_registry: Arc<NodeRegistry>,
    pub pools: Arc<ConnectionPools>,
    pub capture_actual_prompt: bool,
    pub capture_raw_response: bool,
    pub workflows: Arc<HashMap<String, PreparedWorkflow>>,
    pub stream_handler: Option<Arc<dyn StreamHandler>>,
}
```

承载了所有东西：运行时配置、资源池、工作流定义、观测配置、流式回调。

**建议：** 拆分为 EngineContext / WorkflowContext / RunOptions。

### 问题 3：data_sources 定义在 workflow YAML 内部

每个 workflow YAML 里都重复定义 data_sources，10 个 workflow 写 10 遍。

**建议：** data_sources 应支持两层：Engine 级全局注册 + workflow YAML 内可选覆盖。

### 问题 4：Variable Binding 两套语法共存

```
Legacy:  {{node.field}}   — 字符串模板替换，connector 内部解析
New:     {paramName}      — 参数化查询，通过 variables map 预解析
```

造成新用户困惑、SQL 注入风险、IDE 序列化歧义（bindings vs variables bug）。

**建议：** 废弃 Legacy，统一 `variables` + `{paramName}`。

---

## 三、panda-ai-backend 使用分析

### 引擎使用模式
- 使用 `Engine::builder()` API 路径
- 注册 13 个 custom nodes
- 10 个 workflow YAML（3581 行）
- 23 个 prompt 模板文件
- 单 LLM provider (qwen)，单 embedding provider (dashscope)

### 13 个 Custom Nodes

| Node | 用途 | 本应是引擎能力？ |
|------|------|-----------------|
| `stt` | 语音转文字 | 需要通用 HTTP 节点 |
| `tts` | 文字转语音+流式推送 | 需要通用 HTTP 节点 |
| `omni` | Qwen3-Omni 双模态+ffmpeg转码 | 专业领域，合理 |
| `extract_audio` | 提取最佳音频片段 | 专业领域，合理 |
| `voice_enrollment` | 声纹注册 | 专业领域，合理 |
| `dedup_memories` | 向量记忆去重 | 需要 transform 节点 |
| `check_compress_history` | 检查是否需要压缩历史 | 需要 transform 节点 |
| `apply_compress_history` | 执行压缩并写回 Redis | 需要 transform 节点 |
| `check_ad_trigger` | 广告触发判断 | 纯业务逻辑，合理 |
| `red_packet` | 红包逻辑 | 纯业务逻辑，合理 |
| `get_time` | 获取当前北京时间 | 需要内置函数 |
| `image_gen` | 图片生成 | 需要通用 HTTP 节点 |
| `build_json` | JSON 构造 | 需要 transform 节点 |

**核心结论：** 引擎缺少 `transform` 节点和通用 `http` 节点，这两个能消除约 7/13 个 custom node。

### Workflow 模式

**Pattern 1: Sequential Multi-Layer** (最常见)
```
START → 并行预处理 → 搜索/上下文 → 去重/提取 → 主 LLM 调用 → TTS → 存储 → RESPONSE
```

**Pattern 2: Conditional Branching**
```
analyze → condition(has_enough_info) → Path A (搜索+回答) / Path B (追问)
```

**Pattern 3: Sub-Workflow**
```
parent_workflow → workflow(compress_history) → continue
```

---

## 四、节点能力详情

### 引擎节点类型

| Type | Status | Purpose |
|------|--------|---------|
| start | ✅ | 入口，映射输入 |
| generate | ✅ | LLM 调用，支持模板/多模态/流式/结构化输出 |
| response | ✅ | 输出映射 |
| checkpoint | ✅ | 暂停/恢复点 |
| data_connector | ✅ | 数据源操作 (PostgreSQL/Redis/Qdrant/S3/REST/GraphQL) |
| supervisor | ✅ | Agent 规划执行循环 |
| workflow | ✅ | 子 workflow 调用 |
| execute | ❌ | 代码执行 (未实现) |
| mcp | ❌ | MCP 工具连接器 (未实现) |

### 表达式语言（太弱）

当前只支持：
- 比较: `==`, `!=`, `>`, `<`, `>=`, `<=`
- 逻辑: `&&`, `||`, `!`
- 函数: `empty()`, `exists()`, `len()`

**缺失：** 算术运算、字符串操作、类型转换、数组操作

---

## 五、已知 Bug

1. ~~**bindings vs variables**: IDE 序列化为 `config.bindings`，引擎期望 `config.variables`~~ **已修复 (Phase 1)**
2. **Server execute handler**: 缺少 embedding provider 和 WorkflowNode 注册
3. ~~**Legacy variable 语法**: SQL 注入风险，数据类型转换不可靠~~ **已标记 deprecated，加 warn 日志 (Phase 1)**

---

## 六、Flow YAML v1.0 规范（目标）

```yaml
version: "1.0"
name: "workflow_name"
description: "Human readable description"

input:
  fields:
    - name: field_name
      type: string | number | boolean | object | array
      required: true | false
      default: <value>
      description: "Field description"

nodes:
  - id: unique_node_id
    type: <node_type>
    config: { ... }

edges:
  - from: source_node_id
    to: target_node_id
    condition: <expression>
    is_default: true | false
    max_iterations: <number>

templates:
  template_id:
    system_prompt: "..."
    user_prompt: "... {{variable}} ..."
    config:
      temperature: 0.7
      max_tokens: 2048
    output_schema:
      - name: field_name
        type: string | number | boolean | string[] | object | object[]
        description: "..."
```

### Variable Binding 统一规范（只有一种方式）

```yaml
variables:
  # 简单引用
  varName: "node_id.field"

  # 带默认值
  varName:
    from: "node_id.field"
    default: ""

  # 多源优先级
  varName:
    - "node_a.field"
    - "node_b.field"
```

SQL/模板中统一使用 `{varName}` 引用。

---

## 七、开发路线

### Phase 1 — 规范化（做减法） ✅ 已完成
1. ✅ 废弃 `execute()` 公开函数，统一为 Engine API（已删除函数）
2. ✅ 废弃 Legacy variable 语法，只保留 `variables` + `{paramName}`（加 deprecated warn）
3. ✅ data_sources 支持两层：Engine 全局 + workflow 可选覆盖（规范明确，代码不变）
4. ✅ ExecutionConfig 字段分组注释（Engine/Workflow/Run 三层）
5. ✅ 修复 IDE bindings→variables bug（workflow-yaml.ts）
6. ✅ YAML version 升级到 1.0（默认 "1.0"，兼容 "0.1"，加版本校验）
7. ✅ 编写 FLOW_SPEC.md 正式规范文档
8. ✅ 新增 PreparedWorkflow.description 字段
- 93 tests 全部通过，零破坏性变更

### Phase 2 — 补齐核心能力（做加法） 🔜 进行中
1. **transform 节点**：Rhai 嵌入式脚本引擎，纯计算无 I/O
   - `Engine::builder().transforms_dir("./transforms")` 加载 `.rhai` 文件
   - variables 注入为 Rhai 全局变量，返回值 = 节点 output
   - 预注册函数：`now()`, `uuid()`, `json_encode()`, `unique()`
   - 替代 custom nodes: get_time, build_json, dedup_memories
2. data_connector REST 能力增强（自定义 header 模板、multipart、流式响应）
3. 增强表达式语言（算术运算、字符串操作）
4. data_connector 输出标准化

### Phase 3 — Agent 能力增强
1. Supervisor 加强（严格 schema 校验、结构化错误恢复）
2. 实现 MCP 节点
3. Event 系统（节点级 hook）
4. 内置 Memory 能力（RAG pattern）

### Phase 4 — IDE 集成
1. 基于 v1.0 规范开发 IDE 序列化/反序列化
2. 节点面板按规范生成 config 表单
3. 实时 YAML 校验
4. 执行结果可视化
