# Skill Documentation Issues — Round 1

Issues found in `docs/skill.md` during testing. All 8 issues have been addressed.

---

## SI-01: data_connector operation.operation double-nesting [FIXED]
- **Fix**: Added prominent note in Data Connectors section explaining the required double-nesting.

## SI-02: PostgreSQL username vs user [FIXED]
- **Fix**: Added inline comment `# Note: use "username" (not "user")` in the connection config example.

## SI-03: DashScope routing for Zhipu/Moonshot [FIXED]
- **Fix**: Added DashScope routing note in Providers Declaration section.

## SI-04: output_schema reliability not documented [FIXED]
- **Fix**: Added reliability note with provider guidance after the Output Schema section.

## SI-05: on_error edge + response interaction [FIXED]
- **Fix**: Added explanation of on_error routing mechanics in the on_error Edges section. Also fixed the engine bug (EI-09) so on_error paths now work correctly.

## SI-06: Handlebars object interpolation [FIXED]
- **Fix**: Added note about Object/Array auto-serialization in Handlebars Syntax section. Also fixed the engine (EI-08) to pre-serialize objects to JSON.

## SI-07: Rhai vs Rust syntax [FIXED]
- **Fix**: Added "Rhai is not Rust" note with key differences at the top of the transform section.

## SI-08: output_schema placement (template vs node.config) [FIXED — added in Phase 2]
- **Found in test**: [N03], [N04], [S01], [S02], [S35], [S39] — all 29 YAML files had this wrong
- **Problem**: `output_schema` placed under `node.config` is silently ignored because `NodeConfig` has no such field. It must be on the `Template` definition. This was the root cause of all "schema fields null" failures.
- **Fix**: Added WRONG/CORRECT code examples with explicit warning in the Output Schema section. Fixed all 29 test YAML files.
- **Severity**: Critical — this was the single most impactful documentation gap.
