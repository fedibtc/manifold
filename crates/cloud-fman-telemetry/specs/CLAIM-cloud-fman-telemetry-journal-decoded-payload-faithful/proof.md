# Proof: Journal decoded-payload bounds and fidelity

## Scope and model

Scope: `crates/cloud-fman-telemetry/src/journal_types.rs`,
`crates/cloud-fman-telemetry/src/journal_collector.rs`,
`crates/cloud-fman-telemetry/src/archive.rs`,
`crates/cloud-fman-telemetry/src/journal_types_tests.rs`,
`crates/cloud-fman-telemetry/src/archive_tests.rs`,
`crates/cloud-fman-telemetry/Cargo.toml`, `Cargo.toml`, and `Cargo.lock`.

The model quantifies over arbitrary bytes, source incarnation and cursor
values, and continuity flags returned by a production `JournalSession`,
followed by every nonempty `JournalArchive::append` invocation from
`SingleBatchCollector::collect_journals` which returns an `ArchiveFrame`
successfully. The observation is the `frame.start..frame.end` byte range in its
archive path at that append-return point. It does not claim that a later file
read remains unchanged or that the full daily file equals one batch: daily
files concatenate zstd frames.

## Axioms

The three assumptions in
[the claim](../CLAIM-cloud-fman-telemetry-journal-decoded-payload-faithful.md)
are trusted. The zstd premise establishes encode/decode identity for the one
frame supplied to the archive code. The exclusive path access and
local-filesystem premise excludes path mutation throughout the append call and
supplies the external semantics required by the code-derived file-offset and
returned-range argument. The execution-integrity premise covers the reviewed
binary and dependencies. The source's choice of fields other than the
structural marker and top-level span exclusion is deliberately not an axiom
because it is outside this claim.

## Argument

1. **[code] Validation precedes construction.**
   `ValidatedJournalBatch::new` rejects an oversized batch, empty or
   unterminated JSONL, more than 4,096 records, a record over 64 KiB, a line
   which `serde_json::from_slice` does not accept as `Value`, a missing,
   non-boolean, or false `fields.safe_to_share`, and top-level `span` or
   `spans`. It retains the original `Vec<u8>` rather than parsed and
   reserialized values. Its fields are private, and production code has no
   constructor bypass.
2. **[code] The production path passes the same validated bytes to append.**
   `collect_journals` constructs the batch directly from the session response.
   Empty batches return without appending; the nonempty binding is passed
   directly to the sole production `archive.append` call.
3. **[code, assumption] Successful encoding creates one input-equal frame.**
   `JournalArchive::append` gives exactly `batch.jsonl()` to
   `zstd::stream::encode_all` at level 3. Under the zstd premise, its successful
   output is one independently decodable frame whose decoded bytes are the
   validator's retained JSONL.
4. **[code, assumption] The returned range identifies that complete frame at
   append return.**
   One archive mutex covers length reservation through `append_reserved`
   completion. Under the exclusive path access and local-filesystem premise,
   `append_reserved` obtains `start` from the pre-write length, append-writes
   the complete encoded frame, syncs it, computes `end = start + frame.len()`,
   and returns those boundaries. The premise also excludes a waiting operation
   or external actor from replacing the path around open or mutating it before
   the caller's append-return observation. Therefore `[start, end)` in the
   selected archive path contains that frame at the selected return point. This
   range conclusion is derived from the archive code and filesystem premise,
   not assumed as part of zstd behavior.
5. **[test] Focused hostile cases exercise structural admission and retained
   bytes.**
   `rejects_unsafe_spanned_malformed_and_oversized_records` covers false
   markers, spans, malformed JSON, missing newline, and oversized input.
   `accepts_exact_bounded_safe_jsonl` pins retained-byte identity.
6. **[test] Existing archive evidence pins concatenated decoding only.**
   `concatenated_frames_decode_to_exact_source_jsonl` appends two batches and
   decodes the whole concatenated archive to their ordered exact source JSONL.
   It does not slice either returned `[start, end)` range or independently
   decode that slice, so the individual-frame and returned-range joints remain
   `code` plus explicit assumptions rather than a `test` rung.

## Residuals

An event whose own fields are dangerous despite a boolean marker and no
top-level span is outside this structural claim; the upstream safe-event
mechanism owns that classification. Post-return mutation, archive durability
beyond the stated successful filesystem operations, cursor advancement,
retention, recovery, and whole-file equality belong to
[CLAIM-cloud-fman-telemetry-archive-cursor-consistent](../CLAIM-cloud-fman-telemetry-archive-cursor-consistent.md)
where applicable. This claim does not say that the on-disk zstd frame is raw
JSONL.

## Weakest links

The zstd frame contract, local-filesystem behavior, exclusive path access
through append return, and execution integrity are axioms. Validator and append
call-path closure and the returned-range arithmetic remain on the `code` rung.
The existing archive test proves exact concatenated decoding, not independent
decoding of either returned range.
