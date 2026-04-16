# Round 2 Ultimate Test Plan: MySQL, MongoDB, S3/MinIO, Qdrant

## Context

Round 1 测试了 115 个用例（91% 通过率），但 MySQL、MongoDB、S3、Qdrant 四个 connector 因沙箱环境限制全部跳过。现在用户本地有完整的 Docker Compose 环境，需要补全这四个 connector 的详尽测试。

目标：对每个 connector 的**每个操作**、**各种数据类型**、**边界情况**做全面覆盖，复用 Round 1 的 YAML + ut_runner 模式。

## 本地 Docker 连接信息

| 服务 | 容器 | 宿主端口 | 认证 |
|------|------|----------|------|
| MySQL 8.0 | rp-test-mysql | 13306 | user=test, password=test, db=testdb |
| MongoDB 7.0 | rp-test-mongo | 17017 | 无认证 |
| MinIO | rp-test-minio | 19000 (API) / 19001 (Console) | minioadmin/minioadmin |
| Qdrant | rp-test-qdrant | 16333 (HTTP) / 16334 (gRPC) | 无认证 |

## 目录结构

```
ultimate-test/round-2/
  plan/
    TEST_PLAN.md              <- 本计划副本
  workflows/
    mysql/                    <- MySQL 测试 YAML
    mongodb/                  <- MongoDB 测试 YAML
    s3/                       <- S3/MinIO 测试 YAML
    qdrant/                   <- Qdrant 测试 YAML
  results/
    mysql/
    mongodb/
    s3/
    qdrant/
  issues/
    TEST_LOG.md               <- 测试执行日志
    ENGINE_ISSUES.md          <- 引擎问题
    SKILL_ISSUES.md           <- 文档问题
```

## 测试矩阵

### 1. MySQL (M01-M12) — 12 个测试

需要先建测试表（通过第一个测试的 raw SQL 或单独的 setup workflow）。

| ID | 测试名 | 操作 | 覆盖点 | 输入 JSON |
|----|--------|------|--------|-----------|
| M01 | Setup + Query | `query` (CREATE TABLE + INSERT) | 建表、种子数据、raw SQL | `{}` |
| M02 | Basic SELECT | `select` | WHERE/ORDER BY/LIMIT/OFFSET、列投影 | `{"name": "Alice"}` |
| M03 | Parameterized Query | `query` | `{param}` 占位符替换 | `{"search_name": "Bob"}` |
| M04 | INSERT + Readback | `insert` → `select` | 写入→回读 round-trip、RETURNING 模拟 | `{"name":"NewUser","score":88.5,"is_active":true}` |
| M05 | UPDATE + Verify | `update` → `select` | WHERE 条件更新、affected_rows | `{"target":"Alice","new_score":99.9}` |
| M06 | DELETE + Verify | `delete` → `query` | 安全删除（必须有 WHERE）、count 验证 | `{"target":"DeleteMe"}` |
| M07 | NULL Handling | `insert` → `select` | 可空列写入 NULL、读取 NULL | `{"name":"NullTest","score":null}` |
| M08 | Data Types | `query` | INT, FLOAT/DOUBLE, DECIMAL, VARCHAR, TEXT, BOOLEAN, DATETIME, JSON, ENUM | `{}` |
| M09 | Special Characters | `insert` → `select` | Unicode (CJK)、emoji、引号、反斜杠 | `{"name":"测试'用户\"😀"}` |
| M10 | Empty Result | `select` | 不存在的 WHERE 条件，空结果集 | `{"name":"NONEXISTENT"}` |
| M11 | Multiple Rows | `query` | 多行结果、排序验证 | `{}` |
| M12 | INSERT Batch + Count | `insert` × 3 → `query` COUNT | 多次写入、聚合查询 | `{"items":["a","b","c"]}` |

### 2. MongoDB (G01-G14) — 14 个测试

| ID | 测试名 | 操作 | 覆盖点 | 输入 JSON |
|----|--------|------|--------|-----------|
| G01 | Insert One + Find | `insert_one` → `find` | 基本写读 round-trip | `{"name":"Alice","age":30,"city":"Shanghai"}` |
| G02 | Insert Many | `insert_many` | 批量插入、inserted_ids、count | `{"docs":[{"n":"A"},{"n":"B"},{"n":"C"}]}` |
| G03 | Find with Filter | `find` | 条件查询 `{paramName}`、sort、limit | `{"min_age":25}` |
| G04 | Find One | `find_one` | 单文档查询、exists 字段 | `{"name":"Alice"}` |
| G05 | Find One Not Found | `find_one` | 不存在文档、exists=false | `{"name":"NONEXISTENT"}` |
| G06 | Update One | `insert_one` → `update_one` → `find_one` | $set 自动包装、modified_count | `{"name":"UpdateMe","new_age":99}` |
| G07 | Update Many | `insert_many` → `update_many` → `find` | 批量更新、matched_count | `{}` |
| G08 | Delete One | `insert_one` → `delete_one` → `find_one` | 删除验证、exists=false | `{"name":"DeleteMe"}` |
| G09 | Delete Many | `insert_many` → `delete_many` → `count` | 批量删除、count=0 | `{}` |
| G10 | Aggregate Pipeline | `insert_many` → `aggregate` | $match + $group + $sort | `{}` |
| G11 | Count | `count` | 带 filter 的计数 | `{"min_age": 20}` |
| G12 | Nested Documents | `insert_one` → `find_one` | 嵌套对象、数组字段 | `{"user":{"name":"Deep","address":{"city":"BJ","zip":"100000"},"tags":["a","b"]}}` |
| G13 | Special Types | `insert_one` → `find_one` | ObjectId、DateTime、null、boolean、大数字 | `{}` |
| G14 | Projection + Skip | `find` | projection 字段筛选、skip 分页 | `{"skip":2,"limit":2}` |

### 3. S3/MinIO (S01-S10) — 10 个测试

| ID | 测试名 | 操作 | 覆盖点 | 输入 JSON |
|----|--------|------|--------|-----------|
| S01 | Put + Get Text | `put_object` → `get_object` | 文本上传下载 round-trip | `{"content":"Hello S3!","filename":"test.txt"}` |
| S02 | Put + Get JSON | `put_object` → `get_object` | JSON 内容自动解析 | `{"data":{"key":"value","num":42}}` |
| S03 | List Objects | `put_object` × 2 → `list_objects` | prefix 过滤、对象列表 | `{}` |
| S04 | Delete Object | `put_object` → `delete_object` → `head_object` | 删除验证、exists=false | `{}` |
| S05 | Head Object | `put_object` → `head_object` | 元数据检查：size、content_type、last_modified | `{}` |
| S06 | Copy Object | `put_object` → `copy_object` → `get_object` | 跨 key 复制、内容一致性 | `{}` |
| S07 | Presigned URL | `put_object` → `presigned_url` | 生成临时访问 URL、expires_in | `{}` |
| S08 | Overwrite Object | `put_object` → `put_object` → `get_object` | 同 key 覆盖、读取最新版本 | `{}` |
| S09 | Empty/Large Content | `put_object` → `get_object` | 空内容、较大内容(~10KB) | `{}` |
| S10 | List Empty Prefix | `list_objects` | 不存在的 prefix、空结果 | `{"prefix":"nonexistent/"}` |

### 4. Qdrant (Q01-Q10) — 10 个测试

使用小维度(4D)手工向量，不依赖 embedding API。

| ID | 测试名 | 操作 | 覆盖点 | 输入 JSON |
|----|--------|------|--------|-----------|
| Q01 | Create Collection | `create_collection` | 建集合、distance=Cosine、dimension=4 | `{"collection":"ut_test"}` |
| Q02 | Upsert Points | `upsert` | 插入带 payload 的向量点 | `{"points":[{"id":1,"vector":[0.1,0.2,0.3,0.4],"payload":{"text":"hello","category":"greeting"}},...]}` |
| Q03 | Search | `search` | 相似度搜索、limit、score_threshold | `{"query_vector":[0.1,0.2,0.3,0.4],"limit":3}` |
| Q04 | Search with Filter | `search` | must/should/must_not 过滤条件 | `{"query_vector":[...],"filter":{"must":[{"key":"category","match":{"value":"greeting"}}]}}` |
| Q05 | Get Points | `get_points` | 按 ID 获取、with_payload/with_vectors | `{"ids":[1,2,3]}` |
| Q06 | Scroll | `scroll` | 分页遍历全部点、next_offset | `{"limit":2}` |
| Q07 | Delete by ID | `upsert` → `delete` → `get_points` | 按 ID 删除、验证不存在 | `{"delete_ids":[1]}` |
| Q08 | Delete by Filter | `upsert` → `delete` (filter) → `scroll` | 按条件删除 | `{"filter":{"must":[{"key":"category","match":{"value":"temp"}}]}}` |
| Q09 | UUID Point IDs | `upsert` → `get_points` | UUID 格式的 point ID | `{}` |
| Q10 | Upsert Update | `upsert` → `upsert` (same ID, new payload) → `get_points` | 更新已有点的 payload/vector | `{}` |

## 实施步骤

### 第一轮：测试 + 记录问题

**Step 0: 启动 Docker 容器**
```bash
docker start rp-test-mysql rp-test-mongo rp-test-minio rp-test-qdrant
```
等待所有服务 ready（特别是 MySQL 初始化较慢）。

**Step 1: 创建目录结构**
创建 `ultimate-test/round-2/` 下的所有子目录。

**Step 2: 编写 MySQL 测试 YAML (M01-M12)**
- M01 作为 setup，用 raw SQL 建表 + 种子数据
- M02-M12 依赖 M01 建好的表
- 关键文件: `src/nodes/connectors/mysql.rs`

**Step 3: 编写 MongoDB 测试 YAML (G01-G14)**
- 每个测试使用独立的 collection（避免污染）
- 关键文件: `src/nodes/connectors/mongodb.rs`

**Step 4: 编写 S3/MinIO 测试 YAML (S01-S10)**
- 使用独立的 bucket 前缀
- base64 编码内容通过 transform 节点准备
- 关键文件: `src/nodes/connectors/s3.rs`

**Step 5: 编写 Qdrant 测试 YAML (Q01-Q10)**
- Q01 先建 collection，后续测试依赖它
- 使用 4 维小向量，手工构造
- 关键文件: `src/nodes/connectors/qdrant.rs`

**Step 6: 逐个执行测试**
每个测试用 `cargo run --example ut_runner` 执行，结果保存到 `results/` 目录。

执行顺序：MySQL (M01→M12) → MongoDB (G01→G14) → S3 (S01→S10) → Qdrant (Q01→Q10)

**Step 7: 分析结果 + 记录**
- 每个测试结果用 sub-agent 分析（避免大 JSON 污染上下文）
- 记录到 `issues/TEST_LOG.md`
- 发现的引擎 bug 记到 `issues/ENGINE_ISSUES.md`
- 文档问题记到 `issues/SKILL_ISSUES.md`

**Step 8: 暂停 — 和用户讨论发现的问题**

---

### 第二轮：修复（讨论后执行）

**Step 9: 根据讨论结果修复 engine/skill 问题**

**Step 10: 回归验证 + 更新 TEST_LOG.md**

**Step 11: 写 `rp-engine/docs/dev-log-2026-04-15.md`**

## 关键文件

| 文件 | 用途 |
|------|------|
| `src/nodes/connectors/mysql.rs` | MySQL connector 实现 |
| `src/nodes/connectors/mongodb.rs` | MongoDB connector 实现 |
| `src/nodes/connectors/s3.rs` | S3/MinIO connector 实现 |
| `src/nodes/connectors/qdrant.rs` | Qdrant connector 实现 |
| `src/nodes/connectors/mod.rs` | Connector 共用工具(validate_sql_identifier 等) |
| `examples/ut_runner.rs` | 测试 runner，直接复用 |
| `ultimate-test/round-1/workflows/dim2-connectors/d01_postgres_select.yaml` | YAML 模板参考 |
| `ultimate-test/round-1/workflows/dim2-connectors/d02_postgres_insert_select.yaml` | 多节点链式参考 |
| `docs/skill.md` | 文档（可能需要同步更新） |

## 验证方式

1. **单元验证**: 每个 YAML 通过 `cargo run --example ut_runner` 执行，检查 result JSON 的 `_test_meta.status == "Success"`
2. **数据验证**: 对写入→读回的测试，检查 response output 中数据一致性
3. **覆盖率**: 46 个测试覆盖 4 个 connector 的全部操作 + 数据类型 + 边界情况
4. **回归**: 发现 bug 修复后重跑对应测试验证
