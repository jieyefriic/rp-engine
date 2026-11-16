# Audit two inventory exports offline

The sample warehouse exports have different column orders and overlapping SKUs
at different sites. The workflow reads both files, matches by **(site, sku)**,
and produces a reviewable JSON report without a model, service, or database.

From the repository root, run with the bundled files:

```sh
cargo run --example inventory_diff
```

Or pass two of your own UTF-8 exports (each must have the case-sensitive headers
`site`, `sku`, `quantity`, and `description`; other columns are ignored):

```sh
cargo run --example inventory_diff -- path/to/before.csv path/to/after.csv
```

The executable prints the report to stdout. Its only I/O with these files is
reading them. With the sample data the summary is **1 added, 1 removed,
1 changed, 1 conflicting key, 2 unchanged**. The changed item (East, 001) has
quantity `12 → 9` (before record 2, after record 4). West SKU `003` appears
twice in the old export, so the workflow returns both old record numbers and
does not guess which quantity to use.

To adapt the graph:

1. Feed the two strings into [workflow.yaml](workflow.yaml) as `before_csv`
   and `after_csv`. The two `csv_table` nodes parse in parallel. Keep the raw
   texts if you need exact file formatting; the parsed table contains cell values.
2. Change `diff.config.operation.keys` to the columns identifying one entity;
   set `compare` to the fields under audit. You can bypass CSV parsing when
   an earlier node already produces tables.
3. Use `diff.report.summary` for counts, or inspect `changed` and `conflicts`
   for exact source records. Changes in key fields appear as removal/addition.
   Reports contain business data; the CLI does not write a report to disk unless
   you redirect stdout.

See the [workflow spec](../../docs/FLOW_SPEC.md#csv_table--read-a-csv-export)
for dialect, output schema and input limits. Values are not normalized or
converted to numbers; this is exact comparison, not fuzzy entity matching.

```sh
cargo test --test inventory_diff
```
