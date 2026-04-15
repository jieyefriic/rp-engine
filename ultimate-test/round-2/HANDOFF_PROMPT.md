# rp-engine Round 2 测试 — 数据库连接器

## 背景

rp-engine 是一个 YAML 原生的 Agent 工作流执行引擎（Rust）。Round 1 已完成 115 个测试，修复了 12 个引擎问题。Round 2 专注于 Round 1 因环境限制未能测试的**数据库连接器**。

## 你的任务

在本地环境（有 Docker 或原生数据库）中，测试以下连接器的完整能力：

### 需要测试的连接器

1. **MySQL / MariaDB** — `kind: mysql`
2. **MongoDB** — `kind: mongodb`
3. **S3 / MinIO** — `kind: s3`
4. **Qdrant (向量数据库)** — `kind: qdrant`

### 重要：你必须知道的事情

#### 1. YAML 格式陷阱

**`output_schema` 必须放在 template 上，不能放在 node.config 下！** NodeConfig 会静默丢弃未知字段。

```yaml
# 错误 — output_schema 被静默忽略：
nodes:
  - id: gen
    config:
      output_schema:  # ← 这里会被丢弃！

# 正确 — 放在 template 上：
templates:
  my_tpl:
    user_prompt: "..."
    output_schema:  # ← 正确位置
      - name: field1
        type: string
```

#### 2. data_connector 的 operation 格式

operation 块必须包含一个内部的 `operation` 字段指定操作类型，这是双层嵌套：

```yaml
- id: query_node
  type: data_connector
  config:
    data_source_id: my_db
    operation:
      operation: query        # ← 操作类型
      sql: "SELECT * FROM t"  # ← 操作参数
    variables:
      param1: "start.value"
```

#### 3. PostgreSQL 字段名

连接配置用 `username:` 不是 `user:`。

#### 4. data_source 的 kind 值

| Kind | 接受的值 |
|------|---------|
| PostgreSQL | `postgresql` 或 `postgres` |
| MySQL | `mysql` 或 `mariadb` |
| MongoDB | `mongodb` 或 `mongo` |
| Redis | `redis` |
| S3 | `s3` 或 `minio` |
| Qdrant | `qdrant` |

#### 5. 各连接器支持的操作

参考 `docs/skill.md` Data Connectors 章节。关键操作：

**MySQL**: `query`, `select`, `insert`, `update`, `delete` — 格式同 PostgreSQL
**MongoDB**: `find`, `find_one`, `insert_one`, `insert_many`, `update_one`, `update_many`, `delete_one`, `delete_many`, `aggregate`, `count`
**S3**: `get_object`, `put_object`, `list_objects`, `delete_object`
**Qdrant**: `search`, `upsert`, `get_points`, `delete_points`, `create_collection`

#### 6. 测试执行方式

使用已有的测试 runner：
```bash
# 安装依赖后
cargo build --example ut_runner

# 执行单个测试
cargo run --example ut_runner -- \
  <yaml_path> \
  '<input_json>' \
  <output_json_path>
```

引擎需要 provider key 来注册（即使测试不用 LLM）：
```bash
export QWEN_API_KEY="你的key"  # 至少注册一个
```

#### 7. State JSON 分析

**不要自己读 state JSON！** 它很大会撑爆 context。用 sub-agent 分析：
- 检查 `result.status` 是否为 "Success"
- 检查 `result.output` 里的字段是否正确
- 检查 `result.executions` 里各节点是否执行

#### 8. 遇到问题的分类

| 问题类型 | 记录位置 | 说明 |
|---------|---------|------|
| YAML 写法错误 | `SKILL_ISSUES.md` | skill.md 文档缺失/误导 |
| 引擎代码 bug | `ENGINE_ISSUES.md` | 引擎本身的问题 |
| 环境问题 | 跳过 | Docker/网络/权限 |

#### 9. 已知的引擎限制

- PostgreSQL NUMERIC 类型会尝试 f64 再 fallback 到 string（已修复，不再 panic）
- Redis 现在支持 17 种操作（get/set/del/publish/json_*/hset/hget/hgetall/hdel/lpush/rpush/lpop/rpop/lrange/llen）
- 浮点数序列化为 f32 精度（如 0.7 → 0.699999988079071），大部分 API 能容忍

### 测试矩阵

为每个连接器至少测试以下场景：

#### MySQL
- [ ] D06: 基础 SELECT（各种类型：INT, VARCHAR, DECIMAL, ENUM, DATETIME, BOOLEAN）
- [ ] D07: INSERT + SELECT 往返
- [ ] D08: 参数化查询（`{paramName}` 语法）
- [ ] D09: NULL 值处理
- [ ] D10: 中文内容存取

#### MongoDB
- [ ] D11: find 基础查询
- [ ] D12: insert_one + find 往返
- [ ] D13: 嵌套文档 / 数组字段
- [ ] D14: aggregate 管道（$match, $group）
- [ ] D15: 中文内容

#### S3 / MinIO
- [ ] D16: put_object 上传
- [ ] D17: get_object 下载
- [ ] D18: list_objects 列表
- [ ] D19: 不同内容类型（JSON, 纯文本）

#### Qdrant
- [ ] D20: create_collection
- [ ] D21: upsert 向量数据
- [ ] D22: search 相似度搜索
- [ ] D23: get_points 按 ID 查询

### 综合场景（结合 LLM）

- [ ] S-MySQL: MySQL 查询 → LLM 分析 → 报告生成
- [ ] S-Mongo: MongoDB 日志聚合 → LLM 摘要
- [ ] S-RAG: Qdrant 向量搜索 → LLM 回答（RAG 模式）

### 输出要求

1. YAML 文件放在 `ultimate-test/round-2/workflows/` 下
2. 结果 JSON 放在 `ultimate-test/round-2/results/` 下
3. 问题记录在 `ultimate-test/round-2/issues/` 下
4. 每个测试记录：ID、目的、状态（PASS/FAIL）、根因、修改建议

### 参考文件

- `docs/skill.md` — 完整 YAML 编写指南（3500+ 行）
- `ultimate-test/round-1/issues/ENGINE_ISSUES.md` — Round 1 发现的所有引擎问题
- `ultimate-test/round-1/issues/SKILL_ISSUES.md` — Round 1 发现的所有文档问题
- `ultimate-test/round-1/workflows/` — Round 1 的 YAML 范例（可参考格式）
- `src/nodes/connectors/` — 各连接器的 Rust 实现（当行为不符预期时查看）
