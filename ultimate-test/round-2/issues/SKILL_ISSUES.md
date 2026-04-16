# Skill Documentation Issues — Round 2

Issues found in `docs/skill.md` during Round 2 connector testing.

---

## SI-R2-01: S3 variable resolution — doc was correct, engine was wrong [FIXED — engine + doc note]
- **Problem**: `docs/skill.md` already documented `{paramName}` + `variables` for all S3 operations (get_object, delete_object, head_object, presigned_url, copy_object). But the engine only implemented it for `put_object`. The doc-engine mismatch caused user YAML to silently fail.
- **Engine fix**: EI-R2-02 — added `resolve_string_field()` to all S3 operations.
- **Doc update**: Added explicit note "All S3 operations support `{paramName}` variable resolution" after connection config section.
- **Severity**: Critical → Resolved

## SI-R2-02: MySQL type mapping not documented [FIXED — doc]
- **Problem**: `docs/skill.md` MySQL section had no type mapping table. Users couldn't know which MySQL types were supported and how they mapped to JSON.
- **Engine fix**: EI-R2-01 — added DATE/DATETIME/TIME support.
- **Doc update**: Added "MySQL type mapping" table after the "Differences from PostgreSQL" section, covering INT, FLOAT, DOUBLE, DECIMAL, VARCHAR, TEXT, BOOLEAN (→ 1/0), DATE (→ "YYYY-MM-DD"), DATETIME/TIMESTAMP (→ "YYYY-MM-DD HH:MM:SS"), TIME (→ "HH:MM:SS"), JSON, ENUM, NULL.
- **Severity**: Medium → Resolved
