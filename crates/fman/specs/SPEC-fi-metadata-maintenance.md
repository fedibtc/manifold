# SPEC-fi-metadata-maintenance: FI metadata maintenance

## Record justification

The contract spans FI RPC admission, metadata validators, durable occurrence pins, and guardian consensus voting, so no single implementation artifact can own it coherently.

## Decision

The Federation Initiator may request a narrow set of post-formation metadata
changes through the FI-signed, seat-scoped `SetMetaField` verb. Every FMan
independently authorizes the immutable FI/seat binding, validates the key and
value with a compiled validator, preserves every unrelated consensus field,
and casts its own guardian-authenticated meta-module vote. The FI sends the
same typed mutation to every seat and confirms threshold adoption with a fresh
consensus read. An acknowledgement means only that one vote was submitted.

MVP's generic keys are:

| Key | Meaning | Validator |
| --- | --- | --- |
| `federation_name` | Wallet name | At most 65,536 authenticated-input bytes; Guardianito rules trim for validation, require 3–30 bytes, and reject control, bidirectional-control, and zero-width characters plus the case-insensitive `payment request rejected` substring. The original value is submitted. |
| `fedi:federation_icon_url` | Wallet icon | At most 65,536 bytes; trimmed nonempty HTTP(S), at most 2,048 bytes, public host, and no controls. The original URL string is submitted. |
| `fedi:welcome_message` | Wallet description | At most 65,536 bytes; trimmed nonempty value at most 500 bytes with control, bidirectional-control, and zero-width characters refused. |
| `fedi:tos_url` | Terms document | Exactly `https://public.qgcut.org/OG_Federation_ToS.pdf`. |
| `fedi:guardian_fee_send_ppm` | Post-formation fee rate | Canonical decimal in the payer range and at or above the currently published Fedi floor. |

Empty values do not clear fields and unknown keys fail closed. The formation
trust directory and guardian-fee recipient list are formation-owned fixed
fields and are explicitly rejected through `SetMetaField`. Formation installs
both alongside the initial rate through `ProposeFormationMeta`; see
[SPEC-guardian-fee-policy](./SPEC-guardian-fee-policy.md).

## Whole-object merge safety

Fedimint's meta module reaches consensus over one opaque value, not field
patches. Every signed mutation therefore carries `expected_base`, either
`Absent` or a domain-separated SHA-256 over the meta module's monotone
consensus revision and exact raw value from one read. The base names an
occurrence, so byte-identical content readopted at a later revision does not
re-arm an old request.

`SetMetaField` and formation's `ProposeFormationMeta` enter the same FMan
read/check/merge/canonicalize/submit primitive. A mismatch returns
`MetaConsensusChanged` without a vote. A matching generic request replaces only
the validated string field; formation constructs its directory, recipient
mapping, and rate as one target. Both preserve unrelated fields and submit
canonical whole-object bytes.

The upstream module has no conditional submit, so a race remains between the
check and vote. The FI serializes all metadata writers, confirms each by fresh
consensus readback, and recovers by rereading, rebasing, and replaying the same
typed mutation. Within one unchanged base, it retains acknowledgements for a
live invocation and retries only unresolved seats with bounded backoff. A fresh
base resets acknowledgements.

Each live `SeatLoop` also pins the first whole-object target admitted for an
occurrence before its fallible child submit. Exact replay is allowed; a
different target on the same occurrence returns `MetaTargetConflict`. The pin
is process-local and is irrelevant once consensus moves. This prevents delayed
independent Iroh handlers from casting conflicting votes after a newer handler,
at the deliberate availability cost that disjoint sub-threshold writers can
wedge an occurrence until consensus moves or affected FMan processes restart.
A well-behaved FI avoids that residual by serializing writers.

The complete raw consensus object is capped at 1,048,576 bytes. FI checks the
cap immediately after each read, before hashing, parsing, connecting, signing,
or fan-out. FMan checks the live object and canonical target. Formation applies
the same cap before its proposal wave.

## Carried fee-policy validation

A generic write carrying fee fields must carry both the rate and recipient
list. Before voting, FMan verifies the stored canonical seat directory against
the live final config, derives all guardian accounts, and checks the fixed
FI=4, guardian=1, Fedi=1 recipient split plus the rate bounds. The stored
directory does not contain endpoint proofs; those are admission-time inputs to
`ProposeFormationMeta`, not permanent self-verifying evidence. This prevents an
unrelated name or icon update from silently copying a malformed fee policy as
this guardian's vote.

## Failure and security notes

- Signatures cover timestamp, FI and seat ids, exact base, key, and value.
  Formation signatures instead cover the paired attestation/proof entries, FI
  and Fedi fee accounts, and initial rate.
- Formation returns `FormationMetaAlreadyPublished` when a consensus directory
  already exists, distinct from stale base or target conflict.
- Generic keys over 128 bytes, unknown keys, and oversized values are rejected
  before child access. Attacker-controlled key bytes are not logged.
- The icon host check rejects literal/private and obviously local names but does
  not resolve DNS; consumers must enforce fetch-time network policy.
- Maintenance failures are distinct from formation failures. Intrinsic policy
  refusals are terminal; stale bases, temporary seat failures, transport,
  request timeout, and consensus read errors retry under the bounded run.

## Alternatives considered

- **Send complete maps from FI.** Rejected because it widens FI authority to
  unrelated fields.
- **Commit only to bytes or revision.** Rejected because bytes alone re-arm on
  recurrence and revision alone does not bind the content the FI merged.
- **Rely on the RPC's per-field shape.** Rejected because the downstream vote
  is still one whole object.
- **Durable mutation sequences.** More general but require coordinated durable
  counters across FI and every FMan; the occurrence base plus process-local pin
  is proportionate for the serialized MVP writer.
- **Atomic conditional submit.** Preferred if the upstream meta module exposes
  one in the future.
