# Reconstruct sessions from an event log

Run `cargo run --example event_sessions` for the bundled out-of-order log, or
`cargo run --example event_sessions -- events.jsonl` for your own UTF-8 file.
The CLI reads at most 2 MiB + 1 byte, then the parser enforces the limit.
No model keys, services, database or network are used. The JSON report goes to stdout.

Each nonblank line must be an object with string fields `entity`, `timestamp`
(RFC3339 with explicit timezone) and `action`. Convert other log schemas upstream.
The `jsonl_events` node binds `variables.text` and returns `events`, preserving
the original timestamp and physical source line (blank lines count).
Unknown fields are rejected. If a source_line is supplied, the parser overwrites
it with the actual source position.

The `sessionize_events` node binds `variables.events` and uses
`operation.idle_seconds` (required integer 1..86400). It sorts each entity's
events by UTC millisecond time, breaking ties by source line. A gap strictly
larger than the threshold starts a new session. The output `report` contains
entity/event/session counts and sessions with start/end timestamps, duration_ms,
event_count and ordered events. Entities are sorted lexicographically, then
sessions chronologically. Start/end retain the original timezone notation.

The bundled file produces 4 events, 2 entities and 3 sessions. Visitor-a's first
session lasts 120000 ms: browse (line 4), then checkout (line 1). Return (line 5)
starts a second session. Visitor-b has a single-event session of duration zero.

Both nodes reject oversized inputs: 10000 events; entity/action at most 256 UTF-8
bytes, timestamp at most 64. The parser additionally caps raw input at 2 MiB.
Direct normalized event inputs require unique, positive source_line values.
Malformed input fails the whole node; there is no silent skipping or partial report.
An empty log is valid. The nodes run in memory and do not tail files, deduplicate
events or infer causal relationships. Duration is elapsed first-to-last event
time (millisecond precision), not active engagement time. Reports contain raw event
identifiers/actions; use appropriate access controls.

Run `cargo test --test event_sessions` to verify the actual workflow, source
positions, timezone normalization, idle boundaries, deterministic ties and limits.
