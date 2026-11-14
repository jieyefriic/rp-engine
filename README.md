# riceprompt-engine

YAML-native agent workflow execution engine, written in Rust.

You describe an agent workflow as a YAML file — nodes, edges, prompts, data
sources, MCP tools — and the engine parses it, resolves dependencies, and
executes the graph: making LLM calls, running scripts, querying databases,
calling MCP tools, iterating over data, and orchestrating multi-agent plans.

This engine powers **[RicePrompt](https://riceprompt.app)** — the visual
agent IDE where you build and run these workflows without writing YAML by hand.

```toml
[dependencies]
riceprompt-engine = "0.1"
```

## Features

- **YAML-native** — entire workflow (graph, prompts, data sources, providers)
  in a single declarative file. See [`docs/FLOW_SPEC.md`](docs/FLOW_SPEC.md)
  for the authoritative spec.
- **Multi-provider LLM support** — OpenAI, Anthropic, Gemini, DeepSeek, Qwen,
  Zhipu, Moonshot, MiniMax, xAI, Huoshan, and any OpenAI-compatible endpoint.
- **Streaming, tool calling, structured output** — first-class across providers.
- **Rich node types** — `generate`, `transform` (Rhai scripting), `iterator`,
  `supervisor` (multi-agent routing), `subgraph`, `data_connector`,
  `skill_set` (progressive-disclosure knowledge bundles), `mcp` /
  `mcp_tools` (Model Context Protocol).
- **Local document retrieval** — split text into citable passages, search English and Chinese
  with BM25, and optionally fuse a second retrieval ranking.
- **Built-in data connectors** — PostgreSQL, MySQL, MongoDB, Redis, Qdrant,
  S3-compatible object storage, REST APIs.
- **Harness layer** — workflow-level instructions (CLAUDE.md-style) injected
  into every generate node, with persistent memory support.
- **Self-describing results** — `ExecutionResult` can include the source YAML
  so downstream tooling renders the topology + per-node results from one file.
- **Checkpoint / resume** — pause and resume long-running workflows.

## Quick start

A minimal three-node workflow:

```yaml
version: "1.0"
name: "hello_world"

providers:
  openai:
    api_key: "${OPENAI_API_KEY}"

nodes:
  - id: start
    type: start

  - id: greet
    type: generate
    config:
      provider: openai
      model: gpt-4o-mini
      template: tpl_greet
      variables:
        name: "start.name"

  - id: response
    type: response
    config:
      output:
        greeting: "greet.output"

edges:
  - from: start
    to: greet
  - from: greet
    to: response

templates:
  tpl_greet:
    user_prompt: "Greet {{name}} warmly in one sentence."
```

Run it:

```rust
use riceprompt_engine::Engine;
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let yaml = std::fs::read_to_string("hello.yaml")?;
    let engine = Engine::builder().build()?;
    let result = engine.run_yaml(&yaml, json!({ "name": "Ada" })).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
```

Try [offline document retrieval](examples/workflows/local_retrieval.yaml) without a model or database:

```sh
cargo run --example local_retrieval
# Use your own {"documents":[{"id":"...","text":"..."}],"query":"..."}:
cargo run --example local_retrieval -- path/to/input.json
```

The [sample input](examples/workflows/local_retrieval.input.json) asks where to return
library books at night. The first hit is the original passage about the east
entrance, with its document ID and UTF-8 byte offsets. To add existing vector
results, pass a `vector` array ranked best-first using the same chunk IDs.
See the [node contracts](docs/FLOW_SPEC.md#text_chunk--citable-passages).

More runnable examples live under [`examples/`](examples/).

## Documentation

- [`docs/FLOW_SPEC.md`](docs/FLOW_SPEC.md) — authoritative YAML workflow spec
  (node types, fields, providers, data sources, harness, skills, MCP).
- A user-facing usage guide ("skill guide") will be published separately.

## Related

- **[RicePrompt](https://riceprompt.app)** — visual agent IDE built on top
  of this engine. Design workflows in a graph editor, run them in-browser,
  and export the same YAML this engine consumes.

## Project status

`0.1.x` — the API may change between minor versions while the spec
stabilizes. Pin an exact version if you need stability.

## Contributing

Issues and PRs welcome. Please:

- Run `cargo fmt` and `cargo clippy --all-targets` before submitting.
- Add tests for new node types or provider behaviors.
- For changes that touch the YAML surface, update `docs/FLOW_SPEC.md` in
  the same PR.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
