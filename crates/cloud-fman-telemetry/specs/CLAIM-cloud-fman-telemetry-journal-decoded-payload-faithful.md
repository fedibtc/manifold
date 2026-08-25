# CLAIM-cloud-fman-telemetry-journal-decoded-payload-faithful: Journal decoded payload is structurally bounded and faithful

For every nonempty `ValidatedJournalBatch` constructed on the production
`SingleBatchCollector::collect_journals` path for which
`JournalArchive::append` successfully returns, the returned `[start, end)`
archive range at append return contains one zstd frame whose decoded bytes are
exactly the batch's retained JSONL. That JSONL cannot contain a record larger
than 64 KiB, contain more than 4,096 records, omit its trailing newline, or
contain a line which
`serde_json::from_slice` did not accept as a `serde_json::Value`, lacks
`fields.safe_to_share = true`, or has a top-level `span` or `spans` field.

This claim is about the production collector-to-archive boundary. It does not
independently classify fields inside a structurally accepted event as safe;
that remains the upstream safe-journal source responsibility.

## Assumptions

- On success, `zstd::stream::encode_all(input, 3)` yields one independently
  decodable zstd frame whose decoded bytes equal `input`.
- Exactly one active collector/archive owner writes the archive path. No other
  operation by that owner and no external actor mutates, replaces, or removes
  the selected path during the `JournalArchive::append` call. Successful
  local-filesystem path resolution, open, metadata, append-mode `write_all`,
  `sync_data`, and in-process locking operations have their documented
  semantics.
- The process and its dependencies execute the reviewed production Rust without
  memory corruption or code injection.
