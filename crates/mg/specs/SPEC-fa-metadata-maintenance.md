# SPEC-fa-metadata-maintenance: FA metadata maintenance

## Record justification

The contract spans FA RPC admission, metadata validators, durable occurrence pins, and guardian consensus voting, so no single implementation artifact can own it coherently.

## Decision

The Federation Ambassador may request a compiled set of post-formation metadata
changes through the FA-signed, seat-scoped `SetMetaField` and `SetMetaFields`
verbs. Every MG
independently authorizes the immutable FA/seat binding, validates every key and
value with compiled validators, preserves every unrelated consensus field,
and casts its own guardian-authenticated meta-module vote. The FA sends the
same typed mutation to every seat and confirms threshold adoption with a fresh
consensus read. An acknowledgement means only that one vote was submitted.

`SetMetaFields` groups all edits into one whole-object vote against one
`expected_base`. Empty batches, repeated keys, or any invalid field reject the
whole batch before child access. Consensus adopts the combined document, and
the FA confirms every requested value in one fresh read. This grouping does
not make the base check and consensus adoption an atomic compare-and-swap.

The display, terms, and fee keys are:

| Key | Meaning | Validator |
| --- | --- | --- |
| `federation_name` | Wallet name | At most 65,536 authenticated-input bytes; trim for validation, require 3–30 bytes, and reject control, bidirectional-control, and zero-width characters plus the case-insensitive `payment request rejected` substring. The original value is submitted. |
| `fedi:federation_icon_url` | Wallet icon | At most 65,536 bytes; trimmed nonempty HTTP(S), at most 2,048 bytes, public host, and no controls. The original URL string is submitted. |
| `fedi:welcome_message` | Wallet description | At most 65,536 bytes; trimmed nonempty value at most 500 bytes with control, bidirectional-control, and zero-width characters refused. |
| `fedi:tos_url` | Terms document | Caller-supplied URL, with the same HTTP(S), public-host, length, and control-character checks as the icon URL. The original string is submitted. |
| `fedi:guardian_fee_send_ppm` | Post-formation fee rate | Canonical decimal in the payer range and at or above the currently published floor. |

The Fedi app settings are the federation meta keys the Fedi app reads to
change behavior and that an FA may own. Each is shared as
`FederationAppSetting` so the FA pre-validates with the MG's rules. Values are
strings exactly as the app parses them; list values are JSON inside the string.

| Key | App behavior | Validator |
| --- | --- | --- |
| `fedi:autojoin_communities` | Linked communities members join | JSON array of at most 10 distinct invite codes (compared case-insensitively) that Fedi's parser accepts: lowercased bech32 with HRP `fedi:community2` and a V2 payload (author pubkey, UUID, 32-byte base64 key), or HRP `fedi:community` and a V1 payload whose `community_meta_url` passes the icon URL rules |
| `fedi:default_group_chats` | Group chats shown to members | JSON array of at most 20 distinct Matrix room ids: `!` and a nonempty body, at most 255 bytes, without whitespace or controls. Legacy `!opaque:server` and room version 12 server-less ids both pass |
| `fedi:pinned_message` | Compact message on the native Wallet screen | Exactly empty, or the welcome-message rules with control characters refused anywhere |
| `fedi:default_currency` | Display currency | One of the app's `SupportedCurrency` codes or `CurrencyAliases` names; the app mis-renders any other value |
| `fedi:stability_pool_disabled`, `fedi:multispend_disabled` | Feature hidden unless `false` | `true` or `false` |
| `fedi:invite_codes_disabled`, `fedi:new_members_disabled`, `fedi:offline_wallet_disabled`, `fedi:onchain_deposits_disabled` | Feature restrictions | `true` or `false` |
| `fedi:max_balance_msats`, `fedi:max_invoice_msats`, `fedi:max_stable_balance_msats` | App amount limits; Receive is disabled once the balance reaches the balance limit, and for everyone when the invoice limit is `0` | Canonical decimal from 0 through 2,100,000,000,000,000,000 |

The app reads `fedi:<field>` before `<field>`, so these prefixed keys take
precedence over unprefixed legacy values. The exception is group chats: the
app reads `fedi:default_matrix_rooms` and `default_matrix_rooms` first. MG
does not serve either, so on a federation whose metadata only MGs write they
stay absent.

The app settings' raw caps keep their stored size, including JSON string
escaping, under a quarter of the object cap. This does not make every
combination of valid fields fit: padded name, icon, welcome, and terms fields
can exceed the cap by themselves. MG refuses any target over the cap before voting.

The meta module cannot delete a field. Empty values are refused except for
`fedi:pinned_message`, whose empty value hides the message; lists clear with
`[]` and flags with `false`. Unknown keys fail closed.

Keys that steer payments or infrastructure, or that block the whole
federation in the app, are Fedi-operated and not served: `vetted_gateways` (also MG's own payout gateway
preference), `recurringd_api`, `fedi:fixed_exchange_rate`, and the popup and
`federation_expiry_timestamp` fields. Keys with no effect on MG federations,
such as `fedi:social_recovery_disabled`, are not served either. The amount
limits are served including `0`: they stop receives but not sends, and the FA
undoes them by setting a large value.
`fedi:fedimods` is not served because Fedi loads mods from community metadata.

Serving a key is an MG release decision; the metadata request formats do
not change. An MG without a key's validator refuses it as `MetaKeyRefused`.
The FA skips that seat for the rest of the attempt and continues while enough
other seats could reach the threshold. It stops early when key refusals make
that threshold impossible; otherwise it waits for confirmed adoption within
the caller's deadline. A later attempt checks support again. There is no
capability negotiation.

The formation
trust directory and guardian-fee recipient list are formation-owned fixed
fields and are explicitly rejected through both metadata verbs. Formation installs
both alongside the initial rate through `ProposeFormationMeta`; see
[SPEC-guardian-fee-policy](./SPEC-guardian-fee-policy.md).
The sole dedicated post-formation exception is cooperative guardian transfer:
`ReplaceSeatBinding` authenticates the moved guardian's new MG attestation and
endpoint proof, replaces that peer's directory entry and fee recipient, and
preserves every weight, the rate, and unrelated fields
([SPEC-guardian-transfer](./SPEC-guardian-transfer.md)).

Fedi supplies the URL for both ready-made and custom terms. Manifold has no
default terms URL and does not fetch or approve document content.

## Whole-object merge safety

Fedimint's meta module reaches consensus over one opaque value, not field
patches. Every signed mutation therefore carries `expected_base`, either
`Absent` or a domain-separated SHA-256 over the meta module's monotone
consensus revision and exact raw value from one read. The base names an
occurrence, so byte-identical content readopted at a later revision does not
re-arm an old request.

`SetMetaField`, `SetMetaFields`, formation's `ProposeFormationMeta`, and transfer's
`ReplaceSeatBinding` enter the same MG
read/check/merge/canonicalize/submit primitive. A mismatch returns
`MetaConsensusChanged` without a vote. A matching generic request replaces only
the validated string fields; formation constructs its directory, recipient
mapping, and rate as one target; transfer derives only its dedicated replacement.
All preserve unrelated fields and submit
canonical whole-object bytes.

The upstream module has no conditional submit, so a race remains between the
check and vote. The FA serializes all metadata writers, confirms each by fresh
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
wedge an occurrence until consensus moves or affected MG processes restart.
A well-behaved FA avoids that residual by serializing writers.

The complete raw consensus object is capped at 1,048,576 bytes. FA checks the
cap immediately after each read, before hashing, parsing, connecting, signing,
or fan-out. MG checks the live object and canonical target. Formation applies
the same cap before its proposal wave.

## Formation-owned fields during maintenance

The directory and guardian-fee recipient list are absent from the compiled
generic field registry, so both metadata verbs reject requests targeting either
before child access. For an allowed key, MG carries every other field from
the exact observed consensus occurrence unchanged; it does not revalidate or
rewrite formation-owned values as a side effect of an unrelated update.

A threshold-replaced directory or recipient list is a seat-integrity failure,
not a metadata-field admission decision. Detecting that condition and stopping
the guardian remains outside the current maintenance implementation.

## Failure and security notes

- Signatures cover timestamp, FA and seat ids, exact base, and every key/value pair.
  Formation signatures instead cover the paired attestation/proof entries, FA
  and Guardian Verification Fee accounts, and initial rate.
- Formation returns `FormationMetaAlreadyPublished` when a consensus directory
  already exists, distinct from stale base or target conflict.
- Generic keys over 128 bytes, unknown keys, and oversized values are rejected
  before child access. Attacker-controlled key bytes are not logged.
- The icon and terms host check rejects literal/private and obviously local names but does
  not resolve DNS; consumers must enforce fetch-time network policy.
- Maintenance failures are distinct from formation failures. Intrinsic policy
  refusals are terminal; stale bases, temporary seat failures, transport,
  request timeout, and consensus read errors retry under the bounded run.

## Alternatives considered

- **Send complete maps from FA.** Rejected because it widens FA authority to
  unrelated fields.
- **Commit only to bytes or revision.** Rejected because bytes alone re-arm on
  recurrence and revision alone does not bind the content the FA merged.
- **Rely on the RPC's per-field shape.** Rejected because the downstream vote
  is still one whole object.
- **Durable mutation sequences.** More general but require coordinated durable
  counters across FA and every MG; the occurrence base plus process-local pin
  is proportionate for the serialized MVP writer.
- **Atomic conditional submit.** Preferred if the upstream meta module exposes
  one in the future.
