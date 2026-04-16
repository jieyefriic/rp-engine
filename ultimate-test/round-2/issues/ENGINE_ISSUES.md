# Engine Issues — Round 2

Issues found in the rp-engine during Round 2 connector testing (MySQL, MongoDB, S3/MinIO, Qdrant).

---

## EI-R2-01: MySQL DATE/DATETIME types return null [FIXED — verified]
- **Found in test**: [M08]
- **Source file**: `src/nodes/connectors/mysql.rs:437-464` (`row_value_to_json`)
- **Problem**: `row_value_to_json` tries `i64 → f64 → String → bool` fallback chain. MySQL DATE/DATETIME values don't match any of these types, falling through to `Value::Null`.
- **Fix applied**: Added `chrono` feature to `mysql_async` dependency. Added `NaiveDateTime`, `NaiveDate`, `NaiveTime` handlers before numeric types, converting to ISO-8601 string format.
- **Verification**: M08 now returns `date_val: "2026-04-15"`, `datetime_val: "2026-04-15 10:30:00"`.
- **Severity**: Medium → Resolved

## EI-R2-02: S3 read operations don't support `{paramName}` variable resolution [FIXED — verified]
- **Found in test**: [S01], [S02], [S04], [S05], [S06], [S07], [S08], [S09]
- **Source file**: `src/nodes/connectors/s3.rs` — all operations
- **Problem**: `get_object`, `head_object`, `delete_object`, `copy_object`, `presigned_url` used `get_required_string` which only resolves legacy `{{node.field}}` templates. The `{paramName}` + `variables` map pattern was not supported.
- **Fix applied**: Added `resolve_string_field()` and `resolve_optional_string_field()` helpers that resolve `{paramName}` from the variables map, with fallback to legacy `{{}}` resolution. All S3 operations now accept `variables` parameter and use the new resolvers for `bucket`, `key`, `prefix`, `source_bucket`, `source_key` fields.
- **Verification**: New test with `key: "{the_key}"` + `variables: {the_key: "put.key"}` works correctly.
- **Severity**: Medium → Resolved

## EI-R2-03: Qdrant get_points/scroll/search don't include vectors in output [FIXED — verified]
- **Found in test**: [Q05]
- **Source file**: `src/nodes/connectors/qdrant.rs` — `execute_get_points`, `execute_scroll`, `execute_search`
- **Problem**: Point-to-JSON conversion only extracted `id` and `payload`. Vectors were ignored even when `with_vectors: true`.
- **Fix applied**: Added `vectors_output_to_json()` helper that handles both single-vector (`VectorsOptions::Vector`) and named-vectors (`VectorsOptions::Vectors`) formats. Applied to `execute_search`, `execute_scroll`, and `execute_get_points`. Vectors are only included in output when present (non-null).
- **Verification**: Q05 `with_vectors=true` now returns `vectors: [1.0, 0.0, 0.0, 0.0]`; `with_vectors=false` correctly omits vectors.
- **Severity**: Low → Resolved

## EI-R2-04: S3 error messages too generic [FIXED]
- **Found in test**: All S3 tests initially (localhost proxy issue)
- **Source file**: `src/nodes/connectors/s3.rs` — all `map_err` calls
- **Problem**: AWS SDK errors were converted via `e.to_string()` which only returned "service error" with no HTTP status or body detail.
- **Fix applied**: Changed all `e.to_string()` to `format!("{:#}", e)` which includes the error chain (HTTP status, source error, etc.).
- **Severity**: Low → Resolved

## EI-R2-05: MySQL BOOLEAN returns integer (1/0) instead of JSON boolean [INFO — expected]
- **Found in test**: [M01], [M08]
- **Problem**: MySQL BOOLEAN columns return `1`/`0` (integers) rather than `true`/`false`.
- **Root cause**: MySQL stores BOOLEAN as TINYINT(1). The `row_value_to_json` function matches `i64` before `bool`.
- **Note**: Now that temporal types are checked first, the type priority is: NaiveDateTime → NaiveDate → NaiveTime → i64 → f64 → String → bool. The bool handler is technically reachable but MySQL's TINYINT(1) matches i64 first.
- **Severity**: Low (expected MySQL behavior)
