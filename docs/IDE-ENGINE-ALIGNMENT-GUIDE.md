# IDE ↔ Engine 渐进式对齐测试指南

## 概述

本文档记录如何**渐进式**验证 RicePrompt IDE 导出的 YAML 与 rp-engine 是否完全对齐。
从最简单的解析开始，逐步推进到真实执行，每一步都建立在上一步的基础上。

**原则：Engine 是 source of truth，不动 engine。**

---

## Level 0: Engine 已有的 fixture 能跑通

**目的：** 确认 engine 本身没问题，建立 baseline。

```bash
cd rp-engine && cargo test
```

Engine 自带 5 个 fixture（`tests/fixtures/`）：
| 文件 | 覆盖场景 |
|------|---------|
| `linear.yaml` | Start → Generate → Response |
| `conditional.yaml` | 条件分支 |
| `parallel.yaml` | 并行执行 |
| `loop.yaml` | 循环 + max_iterations |
| `TaskPlanner.yaml` | Supervisor 节点 |

如果这里有失败，说明 engine 环境有问题，先解决再继续。

---

## Level 1: IDE 导出 YAML → Engine 能解析（Parse-only）

**目的：** 验证 IDE 输出的 YAML 结构能被 engine 的 serde 反序列化，不需要 API key。

### 步骤

1. **启动 IDE**
   ```bash
   cd RicePrompt && npm run dev
   ```

2. **在 IDE 中构建最简单的 workflow**
   - Start → Generate → Response
   - Generate 节点配置：任意 provider/model、一个 template、一个 output field
   - Response 节点：映射 Generate 的输出

3. **点击 Export 按钮**，下载 `.yaml` 文件

4. **复制到 engine 测试目录**
   ```bash
   cp ~/Downloads/My_Workflow.yaml rp-engine/tests/fixtures/ide_simple.yaml
   ```

5. **写解析测试**（在 `rp-engine/tests/` 下新建或追加）：
   ```rust
   #[test]
   fn test_parse_ide_simple() {
       let yaml = include_str!("fixtures/ide_simple.yaml");
       let workflow = PreparedWorkflow::from_yaml(yaml);
       assert!(workflow.is_ok(), "Parse failed: {:?}", workflow.err());

       let wf = workflow.unwrap();
       // 检查基本结构
       assert!(!wf.nodes.is_empty());
       assert!(!wf.edges.is_empty());
   }
   ```

6. **运行测试**
   ```bash
   cd rp-engine && cargo test test_parse_ide
   ```

### 如果失败了怎么办

解析失败说明 IDE 导出的字段名/结构与 engine schema 不匹配。

**排查方法：**
```bash
# 对比 IDE 导出 vs engine fixture，找差异
diff ~/Downloads/My_Workflow.yaml rp-engine/tests/fixtures/linear.yaml
```

常见问题：
- 字段名不匹配（已在 Phase 1 修复过 `workflow_id→workflow`, `condition→when` 等）
- 多余字段（engine 默认忽略，不应报错）
- 缺少必填字段（如 generate 节点缺 `provider` 或 `model`）

**修复位置：** `RicePrompt/src/lib/workflow-yaml.ts`（不改 engine）

---

## Level 2: 逐类型节点解析验证

**目的：** 确认每种节点类型的 YAML 都能正确解析。

依次在 IDE 中构建并导出以下 workflow，每个单独测试：

### 2.1 最简线性流
```
Start → Generate → Response
```
- 验证：`start` 的 `output`、`generate` 的 `template/provider/model/variables`、`response` 的 `output`

### 2.2 并行流
```
Start → [Generate_A, Generate_B] → Response
```
- 验证：两条 edge 从 start 出发，Response 能引用两个 generate 的输出

### 2.3 条件分支
```
Start → Generate → (condition) → [Response_A, Response_B]
```
- 验证：edge 上的 `condition` 表达式格式

### 2.4 Supervisor + ToolBox
```
Start → Supervisor(toolbox with Generate) → Response
```
- 验证：`subgraphs` 结构、supervisor 的 `toolbox` 引用、子图节点

### 2.5 Checkpoint
```
Start → Generate → Checkpoint → Response
```
- 验证：checkpoint 节点的 `output` 映射

### 2.6 Sub-Workflow（如果已启用）
```
Start → WorkflowNode → Response
```
- 验证：`workflow` 字段（引用另一个 workflow 名称）、`variables` 输入映射、`output` 输出映射

### 每个测试的模板

```rust
#[test]
fn test_parse_ide_LEVEL_NAME() {
    let yaml = include_str!("fixtures/ide_LEVEL_NAME.yaml");
    let wf = PreparedWorkflow::from_yaml(yaml)
        .expect("Failed to parse ide_LEVEL_NAME.yaml");

    // 根据场景验证节点数量
    assert_eq!(wf.nodes.len(), EXPECTED_COUNT);

    // 验证特定节点类型存在
    let node_types: Vec<_> = wf.nodes.iter()
        .map(|n| &n.node_type)
        .collect();
    assert!(node_types.contains(&&NodeType::Start));
    assert!(node_types.contains(&&NodeType::Response));
}
```

---

## Level 3: 简单端到端执行（需要后端 + API Key）

**目的：** 验证 IDE → 后端 → Engine 的完整链路。

### 前置条件

1. 后端运行：
   ```bash
   cd RicePrompt-Server && cargo run
   ```
2. 至少配置一个 provider 的 API key（`.env` 中）
3. 有效的 JWT token（通过登录获取）

### 3.1 用 curl 测试

先用 Level 1 导出的简单 YAML 测试：

```bash
# 读取 YAML 文件内容（转义为 JSON 字符串）
YAML_CONTENT=$(cat rp-engine/tests/fixtures/ide_simple.yaml | jq -Rs .)

curl -X POST http://localhost:3000/api/workflows/execute \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d "{
    \"yaml\": $YAML_CONTENT,
    \"input\": {\"query\": \"What is 2+2?\"}
  }"
```

### 3.2 检查返回

成功时返回 `ExecutionResult`：
```json
{
  "status": "Success",
  "output": { "answer": "..." },
  "executions": { "start": [...], "generate_node": [...], "response": [...] },
  "timing": { "total_ms": 1234, "per_node": { ... } },
  "token_usage": { "total_input_tokens": 50, "total_output_tokens": 100, ... },
  "error": null
}
```

### 3.3 常见错误及排查

| 错误 | 原因 | 修复 |
|------|------|------|
| `Provider 'xxx' not found` | 后端未配置对应 API key | `.env` 中添加 key |
| `Template 'xxx' not found` | YAML 中 template 引用名与定义不匹配 | 检查 IDE 导出的 `templates:` 部分 |
| `No start node found` | YAML 缺少 type: start 节点 | IDE 中确保有 Start 节点 |
| `Variable 'xxx' not resolved` | 变量引用格式不对 | 检查 `variables` 中的表达式（如 `"start.query"`） |
| `401 Unauthorized` | JWT token 无效或过期 | 重新登录获取 token |

---

## Level 4: IDE 按钮直接触发执行

**目的：** 在 IDE 中加 "Run" 按钮，点击后自动导出 YAML 并发送到后端执行。

### 需要添加的代码

**API 层**（`RicePrompt/src/lib/services/api.ts`）：
```typescript
export const executeApi = {
  run: async (yaml: string, input: Record<string, any> = {}) => {
    const res = await fetch(`${API_BASE}/api/workflows/execute`, {
      method: 'POST',
      headers: authHeaders(),
      body: JSON.stringify({ yaml, input }),
    });
    return res.json();
  },
};
```

**UI**（`GraphToolbar.tsx`）：
- 在 Export 按钮旁加一个 "Run" 按钮
- 点击后调用 `workflowToYaml()` 获取 YAML
- 弹出输入框让用户填 start node 的 input
- 发送到后端，显示结果

这一步等 Level 3 全部通过后再做。

---

## Level 5: 高级场景验证

通过 Level 4 后，逐步测试更复杂的场景：

| 场景 | 验证点 |
|------|--------|
| 多 Generate 并行 | 是否真的并行执行，timing 是否合理 |
| Supervisor 循环 | toolbox 是否被正确调用，exit_reason 是否返回 |
| 条件分支 | 不同条件下是否走不同路径 |
| Checkpoint 暂停 | status 返回 `Paused`，checkpoint_state 非空 |
| 循环 + max_iterations | 是否在 max_iterations 后正确退出 |
| Template selector | 条件模板选择是否根据 `when` 条件选对模板 |

---

## 检查清单

用这个清单跟踪进度：

- [ ] Level 0: `cargo test` 全部通过
- [ ] Level 1: IDE 导出最简 YAML → `PreparedWorkflow::from_yaml()` 成功
- [ ] Level 2.1: 线性流解析通过
- [ ] Level 2.2: 并行流解析通过
- [ ] Level 2.3: 条件分支解析通过
- [ ] Level 2.4: Supervisor 解析通过
- [ ] Level 2.5: Checkpoint 解析通过
- [ ] Level 3.1: curl 简单执行成功
- [ ] Level 3.2: 返回结果格式正确
- [ ] Level 4: IDE "Run" 按钮可用
- [ ] Level 5: 高级场景全部通过

---

## 关键文件速查

| 文件 | 用途 |
|------|------|
| `RicePrompt/src/lib/workflow-yaml.ts` | IDE YAML 导出/导入逻辑 |
| `RicePrompt/src/components/tools/graph-builder/GraphToolbar.tsx` | IDE 工具栏（Export/Run 按钮） |
| `RicePrompt/src/lib/services/api.ts` | 前端 API 调用层 |
| `rp-engine/src/schema/workflow.rs` | `PreparedWorkflow::from_yaml()` |
| `rp-engine/src/schema/node.rs` | `NodeConfig` 字段定义（engine 的 source of truth） |
| `rp-engine/src/schema/template.rs` | `TemplateCondition`（`when`/`use`） |
| `rp-engine/src/engine/executor.rs` | `execute()` 函数、`ExecutionResult` |
| `rp-engine/tests/fixtures/*.yaml` | Engine 自带的参考 YAML |
| `RicePrompt-Server/src/routes/execute.rs` | 后端执行端点 |
