# ARCH-fi-client-discovery-selection: FMan discovery and selection

`fi-client` turns an open-write Nostr advertisement enumeration into a
statically admitted candidate set, then into a trusted FMan seat set. Discovery
does cheap local admission and selection performs only the relay-backed
verification and FMan contact needed to fill the requested set. The event schema
is governed by [SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md)
and the badge chain by
[SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md).

## Discovery

`discover_fman_candidates` does not write durable state, take a driver lease, or
change formation status. For each kind-37701 event it checks the event role and
signature, payload proof, `fman_id_pubkey == event author`, newest event per
author (with the NIP-01 lowest-id tie break), freshness, and caller eligibility.
It returns typed rejection reasons as well as candidates, so consumers can
report the observed and eligible counts.

An advertisement is fresh at `now` only when `expires_at > now`,
`issued_at <= now`, and `now - issued_at <= 2h`
(`FMAN_ADVERTISEMENT_MAX_AGE`). The maximum age bounds replay of a
publisher-chosen expiry; it does not establish that advertisement contents are
currently true. Selection checks live availability and quoting checks it again
before a durable effect. The age limit and the FMan publication cadence are
coupled and must be revised together.

A fresh, authenticated advertisement is eligible only when it:

- advertises the requested federation size and a typed scalar `fedimintd`
  version inside the FI's allowed range whose SemVer build metadata is exactly
  `fedi`;
- contains an `InfiniteBestEffort` plan, whose numeric millisatoshi price is the
  advertised estimate; unknown plan families do not prevent use of a supported
  plan;
- contains at least one Holder authorization, since selection reads the claimed
  issuer from the first one; and
- has a parseable x-only `service_pubkey` and at least one parseable
  `iroh://<endpoint-id>` endpoint. These failures are respectively
  `MalformedServicePubkey` and `NoDialableEndpoint`.

The complete enumeration is bounded at 2,048 retained candidates
(`FMAN_ADVERTISEMENTS_CANDIDATE_LIMIT`), 16 MiB retained bytes
(`FMAN_ADVERTISEMENTS_RETAINED_MAX_BYTES`), and the shared 256 KiB per-event
cap (`ROLE_FETCHED_EVENT_MAX_BYTES`). Reaching either retained-prefix bound
returns that prefix rather than letting an open-write publisher turn volume into
a discovery failure. Consequently, discovery is not complete: a later honest
advertisement can be omitted. [CLAIM-fi-client-bounded-discovery-complete](./CLAIM-fi-client-bounded-discovery-complete.md)
records that falsification.

The 16 MiB bound is deliberately lower than the candidate ceiling multiplied by
the per-event cap. The measured worst-case legitimate advertisement event is
7,299 bytes, so 2,048 such events fit within it. **TODO:** tighten the
advertisement-specific per-event cap from that measurement with margin; the
follow-up is beside `FMAN_ADVERTISEMENTS_RETAINED_MAX_BYTES` in
`crates/nostr-clients/src/fi.rs`. A relay response that ends, closes, or times
out before EOSE without reaching a local bound is instead a typed incomplete
query error.

Candidates are shuffled on every discovery run. Advertisements have no capacity
signal, and a stable ordering would concentrate consumers on the same FMan.
Consumers that add their own ranking retain this order as their tie-break.

Discovery constructs a `Locator` from the self-attested commitment-signing
`service_pubkey` and the first usable Iroh endpoint. The Nostr identity and
commitment key use distinct derivations. Path, query, and fragment components
after an endpoint id are ignored; malformed endpoint entries are skipped.
Discovery locators are endpoint-id-only, so network-isolated tests need a
discovery fixture.

## Selection

`preview_fman_selection` groups eligible candidates by the DKG compatibility
identity Fedimint itself uses: major/minor plus exact vendor, ignoring patch and
prerelease differences. FI admits only the `fedi` vendor. It runs selection
independently for every compatible minor line that can fill the federation,
chooses the lowest advertised total, and prefers the newer line when totals
tie. It then buckets that cohort by the
untrusted claimed issuance key, sorts each bucket by advertised setup fee while
preserving discovery order for ties, and fills seats round-robin across buckets. For example, three buckets
`[A, B]`, `[C, D]`, and `[E, F]` yield `A, C, E, B, D, F`. The result is
deterministic for a fixed pool but intentionally changes with the shuffled,
changing pool to favour operator distribution.
The preview rejection summary combines static non-admissions from the complete
enumeration with badge, key, and live failures encountered while producing the
chosen cohort. Failures from incomplete or more expensive cohorts are omitted;
they neither explain nor affect the displayed selection.

Every product-path seat must have a PeerBadge that verifies through a configured
trusted issuer root, meets the environment minimum trust level, binds its Holder
subject to the authenticated advertisement author, and has a verified issuer
equal to the untrusted bucketing issuer. The first verified author reached owns
each commitment-signing `service_pubkey`; another author with that key is
rejected as `DuplicateServicePubkey`. There is no FI-owned, pinned, or BYO
exception in the product path.

Badge verification is deferred from discovery to the ranked walk. For each
reached candidate it examines at most four
(`FMAN_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS`) envelopes and seats the first
one that passes all bindings. A failure or duplicate does not give up the
bucket's turn: it continues until it seats a candidate or exhausts the bucket.
This bounds normal work by reached candidates rather than the whole untrusted
pool, but adversarial cheap candidates can consume the complete deadline before
an honest candidate is verified. The resulting timeout, rather than a late pool
shortfall, is the property in
[CLAIM-fi-client-selection-deadline](./CLAIM-fi-client-selection-deadline.md).

After badge and key checks, a connector-equipped walk calls `GetAvailability`
and applies the same size, allowed range, selected major/minor/vendor identity,
accepting-seats, and plan-family predicate as quoting. Incompatible responses get typed `Live*` rejections; connection,
RPC, and per-candidate-budget failures get `ProbeFailed`. The probe is
price-blind: a live price change is resolved only by the signed quote. The
transport-less `FmanSelectionQuery` intentionally uses advertised claims alone;
the full `FiClient` and replacement preview always probe.

One absolute deadline covers enumeration, admission, badge verification, and
live probes. Expiry wins simultaneous readiness, drops in-flight async work, and
returns `SelectionPreviewTimeout`; it cannot preempt synchronous verifier work
between yield points or executor starvation. Pool exhaustion before expiry is
the typed `InsufficientFmanSeats` partial failure.

## Approval and replacement

The preview is read-only: it returns selected seats, their locators, advertised
prices, verified provenance, estimate, and seen/eligible/selected summary. Its
result is valid for two minutes and can become a non-serializable
`FmanSelectionApproval` that seals the complete request, selected DKG identity,
verifier/environment provenance, locators, estimate, and user cap. Leaving the screen, restarting, or
re-entering obtains a new preview.

`pay_and_create` requests exact quotes only after the consumer supplies that
approval and an authenticated Ready payer. Bootstrap consumes the same approval,
allows a zero limit only for an all-zero estimate, and refuses a priced live
offer before requesting its quote. Pre-output expiry, unavailable selected seats,
quote drift or over-cap, and the wallet's proven pre-journal insufficient balance
return the selected formation to `Idle` with typed reauthorization. Binding,
storage, and ambiguous post-journal errors remain exact recovery.
The approval seals the FI range with its exact Fedi vendor policy and the
selected major/minor/vendor DKG identity,
not one exact build. Quote-time availability must name a typed version inside
both; patch and prerelease changes are accepted, while a minor or vendor change
requires fresh selection before payment.

A terminal payment rejection or settled signed refund makes only that row a
replacement requirement. `preview_fman_replacements` excludes current FMan
identities, treats retained siblings' service keys as occupied, verifies the
fresh subset under the current profile, and seals the row binding and renewed
cap. Applying it preserves the old terminal quote and locator until the new
effect is authorized. It releases an unstarted exact reservation before clearing
its durable id. Siblings that are accepted, paid, prepared, or ambiguous are
never reselected or repaid. A post-output replacement that exceeds its renewed
cap exposes exact `AuthorizePayments`; a pre-output cap failure instead returns
to `Idle`.
Replacement discovery keeps the original FI range but admits only the persisted
DKG identity. Recovery accepts stored or refreshed quotes only when their exact
build remains inside that range and major/minor/vendor identity.
