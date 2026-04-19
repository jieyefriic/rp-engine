# rp-engine Onboarding — for the next agent

## 你在哪 / 读什么

你现在在一个 Rust 项目 `/home/user/rp-engine` 里。这是一个 YAML 原生的 Agent workflow 执行引擎。

**最重要的 3 个文件**：
1. `docs/skill.md`（~3700 行）— 完整 YAML 编写指南，包含所有节点类型、连接器、provider 配置
2. `src/providers/providers.json` — 所有 LLM provider 和模型的 capability 元数据（采样范围、thinking 支持、JSON 模式、function calling、vision 等）
3. `ultimate-test/round-1/issues/ENGINE_ISSUES.md` 和 `ultimate-test/round-2/issues/ENGINE_ISSUES.md` — 我发现并修复的所有引擎问题（17 个）

## 我已经做过的工作（别重复）

### 测试
- **Round 1**：115 个测试，4 维度（10 个 provider × 请求类型、节点能力、真实场景）
- **Round 2**：46 个数据库连接器测试（MySQL/MongoDB/S3/Qdrant 全通过）
- **补充轮**：MCP、harness writeback、thinking、vision、streaming、sub-workflow、checkpoint/resume
- **总计 166 测试，~94% 通过**
- 所有测试记录在 `ultimate-test/round-*/issues/TEST_LOG.md`

### 修复的引擎 bug（17 个）
看 `ultimate-test/round-1/issues/ENGINE_ISSUES.md` 和 `ultimate-test/round-2/issues/ENGINE_ISSUES.md`。摘要：
- PostgreSQL/MySQL 类型 panic（改用 try_get）
- node timeout 无效（wrap 错地方）
- output_schema 字段 null（提供默认值）
- Handlebars 对象渲染成 `[object]`（预序列化）
- on_error edge 下游读不到 fallback 输出（`inputs_satisfied` 忽略 dead edge）
- Redis 缺 hash/list 操作（补了 10 个）
- S3 变量解析只支持 put_object（扩展到所有操作）
- Qdrant 向量不输出（补了 `vectors_output_to_json`）

### Provider 迁移
**重点**：Zhipu 和 Moonshot 之前硬编码走 DashScope（阿里云），我已经改成原生端点：
- Zhipu → `https://open.bigmodel.cn/api/paas/v4/chat/completions`（官方 GLM-4/5/5.1 API）
- Moonshot/Kimi → `https://api.moonshot.cn/v1/chat/completions`（官方 Kimi API）
- MiniMax 原本就是原生端点，没动
- Huoshan/豆包 走火山引擎 `https://ark.cn-beijing.volces.com/api/v3/chat/completions`

## 关键踩坑点（别再踩）

### 1. YAML 陷阱

**`output_schema` 必须放在 template 上，不能放在 node.config 下！**
`NodeConfig` 的 serde 会**静默丢弃**未知字段（没有 `deny_unknown_fields`），所以放错地方不报错但不生效。skill.md 里有 WRONG/CORRECT 对比。

```yaml
# 错误
nodes:
  - id: gen
    config:
      output_schema: [...]  # ← 被静默丢弃

# 正确
templates:
  my_tpl:
    user_prompt: "..."
    output_schema: [...]    # ← 这里才对
```

### 2. data_connector 双层 operation

```yaml
operation:
  operation: query          # ← 类型
  sql: "SELECT ..."         # ← 参数
```

### 3. PostgreSQL 用 `username:` 不是 `user:`

### 4. 中国 provider 的错误码不标准
- 智谱限流会返 `400 - 参数有误` 而不是 429/503
- DeepSeek 限流会返 `503 - DNS cache overflow`
- Moonshot kimi-k2.5 只接受 `temperature=1`（已在 providers.json 加 `maxTemperature:1`）

## 测试方法论（必读）

### 用 sub-agent 分析 state JSON

**不要自己读完整 state JSON**！它很大（几千行），会撑爆你的 context window。用 sub-agent（Haiku 或 Sonnet）读，让他回报关键字段：

```
Agent({
  subagent_type: "Explore",
  model: "haiku",
  prompt: "Read <path>. Report: 1) status 2) output.xxx field 3) any errors. Under 150 words."
})
```

### 用 ut_runner 跑 YAML 测试

已经有写好的 runner：
```bash
cargo run --example ut_runner -- <yaml> '<input_json>' <output_json>
```

环境变量需要 export 各 provider 的 API key。批量脚本在 `ultimate-test/round-1/runner/run_batch.sh`（里面有所有 key）。

### 用 ut_advanced 测 streaming/sub-workflow/checkpoint

这些需要程序化接口，不能纯 YAML：
```bash
cargo run --example ut_advanced -- streaming <output>
cargo run --example ut_advanced -- sub_workflow <output>
cargo run --example ut_advanced -- checkpoint_resume <output>
```

## Provider 架构（如果你要改 provider）

### 核心抽象
- `src/providers/mod.rs` — `LLMProvider` trait、`ChatOptions`、`ChatResponse`、`TokenUsage`
- `src/providers/openai_compatible.rs` — 基底实现（Zhipu/Moonshot/MiniMax/Huoshan/DeepSeek/XAI/Qwen 都基于这个）
- `src/providers/anthropic.rs`、`gemini.rs` — 独立实现（协议不兼容 OpenAI）
- `src/providers/model_registry.rs` — 根据 providers.json 计算模型能力 + 自动屏蔽不支持的参数

### 新增模型的正确姿势
1. 改 `src/providers/providers.json` 加入模型元数据
2. 如果模型有特殊采样约束（如 kimi-k2.5 只能 temp=1），加 `samplingOverride`
3. 如果模型有 thinking 能力，加 `thinking: "optional"|"builtin"` 和 `thinkingBudget` 配置
4. 跑 P01-P10 系列测试验证
5. 更新 `docs/skill.md` Providers 节

### 新增 provider 的正确姿势
1. 如果是 OpenAI 兼容，新建 `src/providers/<name>.rs` 包装 `OpenAICompatibleProvider`
2. 如果协议不兼容，仿照 `anthropic.rs` 或 `gemini.rs` 实现 `LLMProvider` trait
3. 在 `src/providers/mod.rs` 注册
4. 在 `src/engine/builder.rs` 的 `create_provider_from_def` 里加 match arm
5. 在 `providers.json` 加 provider 定义
6. 在 `docs/skill.md` 文档

## 还没测试的维度

记录在 `ultimate-test/round-1/issues/TEST_LOG.md` 末尾："Status of Previously-Untested Dimensions"。摘要：
- GraphQL 连接器（没公共端点）
- OpenAI Compatible 自定义端点（没本地 LLM）
- Embedding 节点
- 并发工作流执行（需要 Rust 测试代码）
- 大规模图（20+ 节点）
- 负载/压力测试

## Git 工作流

- 主分支：`main`
- 所有改动都要 commit + push
- 提交消息用 `type: description` 格式（feat/fix/test/docs/refactor）
- Commit message 末尾带 `https://claude.ai/code/session_xxx`

## 模型选型建议

### 最稳定便宜的工作模型
- **DeepSeek** `deepseek-chat` — 便宜、稳定、中文好（但偶发 503）
- **Qwen** `qwen-flash` — 最便宜、中文好、速度快
- **OpenAI** `gpt-4o-mini` — 英文场景最稳

### 工具调用
- OpenAI、Anthropic、Qwen、DeepSeek 都 OK
- Qwen 有坑：tool 循环用完 max_rounds 没最终响应会空输出（已修，但还是建议用别的 provider）

### 思维链 / 推理
- Anthropic Claude（`thinking_budget`）
- XAI Grok（内置）
- DeepSeek Reasoner
- OpenAI o-series（`reasoning_effort`）

### 多模态
- OpenAI `gpt-4o-mini`（最稳）
- Anthropic Claude
- Gemini `gemini-2.0-flash`
- Qwen `qwen-vl-plus`

## 最后：如果你接手的任务是 model research

你可能要做：
- 研究新模型的 API 和 capability
- 更新 `providers.json`
- 验证新 provider / 新模型
- 写对应的测试

**建议流程**：
1. 先 WebFetch 去看官方文档（API endpoint、auth 方式、请求格式、模型列表）
2. 用 curl 直接测一下确认 key 和端点工作
3. 改 `providers.json`，对齐模型能力字段
4. 如果新 provider，按上面"新增 provider 的正确姿势"走
5. 写 YAML 测试（放 `ultimate-test/round-3/workflows/` 下，模仿 round-1 和 round-2 的结构）
6. 用 ut_runner 跑
7. 结果用 sub-agent 分析

祝好。有问题看 skill.md，再看 ENGINE_ISSUES.md，基本能覆盖 90% 的坑。
