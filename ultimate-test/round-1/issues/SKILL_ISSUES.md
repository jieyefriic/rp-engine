# Skill Documentation Issues — Round 1

Issues found in `docs/skill.md` during testing. Each entry describes what was misleading, incorrect, or missing in the documentation that led to YAML authoring errors.

---

## SI-01: data_connector YAML format — operation.operation vs operation.query
- **Found in test**: [D01], [D02], [D04], [D13]
- **Skill.md section**: Data Connectors > PostgreSQL > Operations
- **Problem**: The nested `operation.operation` field name is confusing and easy to get wrong. Initial attempt used `operation.query` (just the SQL) without the inner `operation: query` type specifier. The skill.md shows the correct format but the double-nesting of `operation.operation` is a common stumbling block.
- **Suggestion**: Add a prominent note or warning: "The `operation` block must contain an `operation` field specifying the type (query, select, insert, etc.) plus type-specific fields."
- **Severity**: Medium

## SI-02: PostgreSQL config field `username` vs `user`
- **Found in test**: [D01]
- **Skill.md section**: Data Connectors > PostgreSQL > Connection config
- **Problem**: skill.md shows `username:` but some users might try `user:`. The engine code uses `ds_config.get("username")` specifically. This is documented correctly but could benefit from a note about the exact field name.
- **Suggestion**: Add a note: "Use `username:` (not `user:`)."
- **Severity**: Low

## SI-03: Provider section missing DashScope routing info for Zhipu/Moonshot
- **Found in test**: [P07_R1], [P08_R1]
- **Skill.md section**: Providers Declaration
- **Problem**: The providers section lists Zhipu and Moonshot but does not mention that they are routed through DashScope. Users providing native API keys will get silent authentication failures. The information is only in the Rust source code comments.
- **Suggestion**: Add to the providers table: "Zhipu: Routed via DashScope (requires DashScope API key, not native Zhipu key)" and similarly for Moonshot.
- **Severity**: High

## SI-04: output_schema reliability not documented
- **Found in test**: [N03], [N04], [S01], [S02], [S35], [S39]
- **Skill.md section**: Templates > Output Schema
- **Problem**: skill.md presents output_schema as a straightforward feature but does not warn about reliability differences across providers. With providers that lack native JSON mode (e.g., DeepSeek in some modes), the LLM may return prose instead of JSON, causing individual schema fields to be null. The engine falls back to `extract_json_from_content` but this is not always successful.
- **Suggestion**: Add guidance:
  1. "output_schema works best with providers that have native JSON mode (OpenAI, Gemini, Qwen)"
  2. "For other providers, the engine attempts to extract JSON from the response via regex. This may fail if the LLM returns pure prose."
  3. "Always test structured output with your specific provider/model combination"
  4. "Consider using `gen.output` as fallback when schema fields may be null"
- **Severity**: High — this is the #1 issue causing workflow failures in testing

## SI-05: on_error edge + response node interaction not documented
- **Found in test**: [N52], [S17]
- **Skill.md section**: Edges & Routing > on_error Edges
- **Problem**: skill.md shows on_error edge syntax but does not explain the interaction with response nodes. When a fallback node runs via on_error edge, its output may not be readable by the response node. This is not documented.
- **Suggestion**: Add a section explaining the output mapping limitations of on_error paths, with a recommended workaround pattern.
- **Severity**: Medium

## SI-06: Handlebars object interpolation behavior not documented
- **Found in test**: [S16], [S06]
- **Skill.md section**: Templates > Handlebars Syntax
- **Problem**: When a template variable contains a JSON object (from a transform node), Handlebars renders it as `[object]` or `[object Object]` instead of serialized JSON. skill.md doesn't mention this behavior or provide a workaround.
- **Suggestion**: Add: "Non-primitive variable values (objects, arrays) are rendered as `[object]` by default in Handlebars. To pass structured data through templates, use `json_encode()` in a transform node first, or access individual fields with `{{var.field}}` syntax."
- **Severity**: Medium

## SI-07: Transform Rhai syntax not distinguished from Rust
- **Found in test**: [N21]
- **Skill.md section**: Node Types > transform
- **Problem**: No clear guidance that Rhai is NOT Rust. Rhai looks like Rust but lacks turbofish syntax, `.collect()`, and many Rust-specific constructs. Easy to write Rust-style code that fails at runtime.
- **Suggestion**: Add a brief note: "Rhai looks like Rust but is a separate scripting language. Key differences: no turbofish `::< >`, no trait methods like `.collect()`, limited type system. See Rhai documentation for supported syntax."
- **Severity**: Low
