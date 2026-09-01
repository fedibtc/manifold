# ARCH-fi-cli: Federation Initiator CLI

`fi-cli` is a development/test-only thin terminal consumer of
[`fi-client`](../../fi-client/specs/ARCH-fi-client.md); its review scope is
constrained by [GATE-fi-cli-test-tool-scope](GATE-fi-cli-test-tool-scope.md).
It owns a persistent
local identity, opens a Fedimint RocksDB database, translates flags and TOML
into `FormationIntent`, subscribes to library progress, and renders results.
Its `payment-wallet` commands join and directly fund the same persistent
Fedimint client wallet later supplied to formation; this is consumer capability
plumbing and does not move selection, authorization, or formation policy out of
`fi-client`.
TOML intent files reject unknown fields and invalid name, size, fee, and
spending-cap values
before the CLI accesses local identity or durable FI state, or opens payment and
network capabilities. `--max-total-msats` (or the TOML `max_total_msats`)
sets the intent's aggregate spending cap; under it `fi-client`
self-authorizes only the initial paid quote set when its checked total fits.
An absent or exceeded cap parks that initial set, and every replacement after
any prior authorization parks regardless of its total.
The global `--manifold-environment` option selects the canonical shared
PeerBadge issuer-root, authority-relay, and minimum-trust-level profile.
Commands that make trust decisions construct its concrete verifier and pass it
directly into stateful `FiClient::open` calls or the state-free
`FmanSelectionQuery`; an authentic badge below that environment minimum is not
a verified candidate. Static `discover` constructs only `FmanRegistryQuery`
and therefore remains usable when an environment has no configured issuer
roots. Because this CLI is development/test-only, it defaults to `development`
([ARCH-manifold-environment](../../manifold-environment/specs/ARCH-manifold-environment.md),
[SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
It has no protocol, trust, signing, or lifecycle implementation of its own —
`fi-client` owns every FMan verb, signature envelope, response verification,
transition, and checkpoint.

The CLI implements the concrete Iroh connector and Fedimint wallet
payment/refund port for both formation modes. `fi-client` owns payment
scheduling for initial formation, resume, and replacement: it starts one new
member at a time and checkpoints the completed seat before the next. The CLI
adapter provides only exact aggregate reservation and operation/recovery
capabilities. Its aggregate holds independently dry-run logical net costs
rather than disjoint notes, re-quotes each transaction's current net debit
under the wallet spend guard, persists the exact Fedimint operation and change
range atomically, and returns only after consensus acceptance and spendable
payer change.

Omitting `--locator` selects the product path: before opening identity, FI
state, or the payment wallet, the CLI binds its Iroh endpoint, runs a fresh
verified preview against the selected environment with live availability
probing over that endpoint, seals it with the command's spending cap, retains
the non-serializable approval in-process, and immediately consumes it through
`pay_and_create`. Registry-backed paid creation therefore requires
`--max-total-msats`; passing no wallet coordinates explicitly selects
`create_without_payer`, which succeeds only when the advertised and live exact
offers are all zero. Supplying locators retains the diagnostic pinned path used
by lower-level protocol E2E tests. That path accepts a paired 0600
completion-callback URL file and idempotency key and delegates protocol
validation, durable ownership, and delivery entirely to `fi-client` and the
pinned FMans; the CLI never places the URL bearer in argv, persists it, or
emits it.
For temporarily incomplete development/staging trust infrastructure,
`--insecure-skip-fman-trust` explicitly discovers authenticated, fresh,
compatible advertisements through `insecure_discover_untrusted_pinned_fmans` and feeds
their locators into that pinned driver. It cannot be combined with `--locator`,
is rejected for the production profile, and never creates a verified selection
or trust claim.

`discover` and `preview` are read-only registry commands: they connect a
pooled Nostr relay client to every canonical relay in the selected
environment profile under ephemeral keys (the enumeration publishes
nothing; one reachable relay suffices), build the
library's `NostrFiClient`, and run `discover_fman_candidates` through the
registry-only `FmanRegistryQuery`; `preview` adds the environment verifier with
`with_verifier` and an ephemeral Iroh FMan connector with
`with_fman_connector`, and runs through `FmanSelectionQuery`, so its selection
walk probes each reached candidate's live availability before seating it.
Neither command owns a payment port; they do not load an FI identity, open
RocksDB, initialize consensus connectors, or touch `--state-dir`. Both take the
same `--timeout-secs` flag, mapped to the library's clamped
`FmanDiscoveryOptions` deadline, with zero
rejected at the argument boundary like the pinned-driver timing flags. Both
render the candidates or selected seats plus the library's
typed rejection summary; neither writes durable formation state nor takes the
driver lease. `discover` contacts no FMan; `preview`'s only FMan contact is
the walk's value-free availability probe. `preview` targets the
`InfiniteBestEffort` plan, matching what discovery eligibility surfaces.

Before accessing local identity or opening or changing a payment wallet, the CLI
validates the inputs applicable to that command. Formation commands additionally
run the matching operation-specific library preflight, validate wallet
coordinates and the payment invite's federation identity, and complete the
selected path's verified preview. A failure from checks before the preview
stage cannot create or read identity, join or fund a wallet, move a funding
token into its journal, initialize FI durable state, acquire the driver lease,
bind the network endpoint, or contact an FMan. The selected path's preview
itself runs after the ephemeral endpoint is bound and contacts FMans only
through the walk's value-free availability probe; a preview failure still
creates no identity, wallet, or durable FI state.

`create`, `resume`, and `authorize-payments` can import an explicitly supplied
funding token after joining the matching payment federation. This lets a parked
paid formation acquire test funds without discarding its durable quote state.
Mint-v2 import and replay recover the deterministic receive operation and wait
for its exact reissue outputs to become spendable before payment preflight.

`payment-wallet join` accepts a setup-payment federation invite, including an
API-secret-bearing invite for a private test federation, initializes its
Fedimint client database beneath `--wallet-data-dir`, and prints the federation
id. Later `payment-wallet` commands reopen that database by the printed id and
the same wallet root secret; they do not need the invite and cannot initialize
an unjoined federation. `balance` reports the spendable
Bitcoin-unit balance. `deposit-address` prefers wallet-v2 and otherwise uses
the legacy wallet module; its wallet-v2 address derivation has an explicit
caller-selected deadline. `wait-balance` waits for the primary Bitcoin module
to reach an explicit minimum. `invoice` prefers LN-v2, falling back to legacy
LN only when LN-v2 reports that no gateway is available before it creates an
operation, and prints both the BOLT11 invoice and its durable Fedimint operation
id. `await-invoice` reopens the client after any process restart, dispatches by
that operation's durable module kind, and waits for its terminal state.
`remit-guardian-fee` is the development-only payer-wire hook: it accepts an
explicit BtcDepositor account id, amount, and pre-sealed metadata, then awaits
the real stability-pool deposit operation. It deliberately does not implement
production payer accrual or recipient policy. It runs under the wallet-wide
spend guard and refuses to consume any locked-payment hold or active reserve
floor. Terminal streams are drained into Fedimint's durable outcome cache; a
post-acceptance change-output failure reports the committed operation id and
explicitly forbids retry. The wallet registers mint-v1,
mint-v2, wallet-v1, wallet-v2, LN-v1, LN-v2, and stability-pool so a consumer
can directly fund both current manifold federations and mixed-version
setup-payment federations during staging.

These commands are deliberately outside `fi-client`: joining and funding a
consumer-owned payer are capability preparation, not FI formation policy.
Registry `preview` remains wallet-free and read-only. Registry-backed `create`
performs a new verified preview, retains the approval only in-process, reopens
the already-funded payer by federation id, and supplies it to
`FiClient::pay_and_create`.

For a paid pinned `create` without an applicable cap, `fi-client` first stops at
`AwaitingPaymentReadiness`. When no cap exists, the CLI prints the
library-provided aggregate requirements and explicitly authorizes that exact
quote set before continuing the same formation in a new driver invocation.
An over-cap initial aggregate remains parked; the CLI prints
`paymentAuthorizationRequired` and requires a distinct `authorize-payments`
command carrying the displayed authorization id and the same wallet
coordinates. An under-cap initial aggregate self-authorizes inside
`fi-client` without this CLI step. Every replacement after any authorization
also remains parked for `authorize-payments`. `resume` never
infers fresh payment authorization — it only continues work covered by an
already durable one. Paid resume reopens the same wallet database and root
secret; the wallet recovers the exact quote-bound Fedimint operation and its
required persisted change range before waiting for finality. It
deterministically reconstructs refund material, so `fi-cli` persists no payment
signatures, ecash, or refund secrets in FI state.

## Post-formation maintenance

`maintenance set-name`, `set-icon-url`, `set-welcome-message`, and
`set-terms-of-service` expose the four semantic metadata mutations currently
supported by `fi-client`. The value-bearing commands construct the library's
shared Guardianito-compatible types before opening identity, durable FI state,
the consensus reader, or Iroh transport. The CLI accepts no arbitrary metadata
key, clear operation, uploaded icon bytes, or alternative ToS URL.

`create` requires `--fi-spv2-account-file PATH` for the test FI account that
formation installs at weight four; `resume` and `authorize-payments` accept the
same option so an interrupted formation can finish the atomic metadata
proposal. This explicit account remains development/test tooling: supplying
another structurally valid account redirects the FI share. The Guardian
Verification Fee account comes from the selected Manifold profile, and
guardian accounts come from signed seat acceptances.

`maintenance configure-guardian-fees [--send-ppm PPM]` changes only the rate of
the recipient policy already installed by formation. It accepts no FI,
guardian, or Guardian Verification Fee account input. Omitting the rate uses
5,000 ppm; fi-client also enforces the admitted published minimum with a
1,500-ppm fallback.

Every maintenance command reopens the existing active formation, reconciles it
through `fi-client::resume`, and invokes the typed library operation through the
same open client and real consensus reader. A failed reconciliation prevents
the mutation future from being polled. Reopening does not connect the live
registry because neither reconciliation nor these operations uses it. The
commands require a durable `Formed` record and return only after fi-client
rereads threshold consensus and observes the exact metadata value or requested guardian-fee rate. They do not persist a second CLI-side operation record.

fi-client does not expose a standalone projection of arbitrary consensus
metadata. Accordingly, fi-cli does not duplicate the private recipient-policy
derivation or versioned fee metadata merely to offer a raw inspection command.

fi-client's public `register_gateway` operation is not a separate fi-cli
command. Existing liquidity start/resume owns gateway attachment and invokes
that operation as part of the same durable liquidity workflow, so a standalone
consumer entry point would duplicate sequencing without exposing another FI
capability.

## Post-formation liquidity

`liquidity discover` performs the complete no-private-data FLIP admission walk
for an explicit Bitcoin network and gateway amount range. `liquidity request`
repeats that fresh discovery, chooses either the named admitted provider or the
first deterministic provider, requires the active formation to be `Formed`,
reconciles it through `fi-client::resume`, and invokes
`fi-client::start_liquidity` in the same process through a generated Iroh
public-RPC client. The CLI never constructs provider trust or sends the invite
itself.

`liquidity resume --operation-id` reopens the same FI database, reconciles the
formed federation through `fi-client::resume`, and asks `fi-client` to
status-query or exact-replay the durable operation in the same process. The
storage-only operation lookup occurs after the environment clients open but
before FMan/FLIP reconciliation, so a malformed or absent operation id cannot
start protocol recovery.
`liquidity status` and `liquidity list` are storage-only projections suitable
for E2E assertions and launch-time recovery. Provider choice is not durable CLI
state: the exact provider, request hash, endpoint hint, amounts, response, and
item statuses are the library-owned liquidity journal governed by
[`SPEC-fi-post-formation-liquidity`](../../fi-client/specs/SPEC-fi-post-formation-liquidity.md).

Identity and state persist in `--state-dir`; secret-key bytes are never
printed or logged. This disposable local state supports development and E2E
restart testing; it is not a production persistence boundary or hardened
against hostile local filesystem behavior.

`--json` is a stable, newline-delimited output contract. Successful `init`
writes exactly one stdout object with `fiPubkey` and `state`; `status`, `create`,
`resume`, and successful `authorize-payments` write exactly one stdout `FiStatus`
value. A formation intent carries `fedimintd_versions` as its inclusive-minimum,
exclusive-maximum range and `fedimintd_version_core` as the release selected for
that DKG. When a paid formation
reaches payment readiness, `create` additionally writes exactly one stderr
object with the sole top-level field `authorizingPayments`, whose value is the
library-provided `PaymentRequirements`. Successful `discover` and `preview`
write exactly one stdout object carrying the `seen`/`eligible` (and, for
`preview`, `selected`, `fedimintdVersionCore`, and `totalAdvertisedMsats`) summary with the
candidate or seat list and typed rejection reasons rendered as strings.
Rejection and provenance strings are explicit lower-snake-case machine codes
owned by `fi-client` (`expired`, `badge_rejected`, `fedi_attested`, and so on),
never Rust `Debug` output. Discovery candidate rows contain
`fmanPubkey`, numeric advertised price/capacity/version fields, `claimedIssuer`,
raw `apiEndpoints`, `locator`, `issuedAt`, and `expiresAt`; preview seat rows
contain `fmanPubkey`, numeric advertised price in millisatoshis, `locator`,
verified issuer/holder/trust level, and provenance. An over-cap
park writes exactly one stderr object with the sole top-level field
`paymentAuthorizationRequired` carrying the same `PaymentRequirements` shape.
Successful commands otherwise leave
stderr empty. Every successful maintenance subcommand writes exactly one stdout
object and leaves stderr empty. Metadata writes use
`{field,value,consensusReached:true}` with the exact consensus key and accepted
value. Guardian-fee rate configuration uses
`{sendPpm,consensusReached:true}` after `propose_guardian_fees` confirms fresh
consensus readback. Every successful liquidity subcommand likewise writes exactly
one stdout JSON value: discovery writes `{providers,rejected}`, request/resume/
status write one `LiquidityOperationSnapshot`, and list writes one
`LiquidityOperationPage`. Human-readable output may change independently.
Every successful `payment-wallet` command likewise writes exactly one stdout
JSON object and leaves stderr empty: join/balance expose `federationId` and
`balanceMsats`; `accounting` reopens the wallet and exposes `federationId`,
`balanceMsats`, `receivedInputMsats`, `receiveFeeMsats`, `setupOutputMsats`,
`setupFeeMsats`, and `setupTransactionCount`. Those accounting fields are
derived from accepted Fedimint receive/setup transactions after validating the
persisted exact setup output/change ranges and awaiting the same payer-change
finality primitive as adapter recovery; the command is diagnostic and never
creates, replaces, releases, or otherwise schedules formation payments.
Deposit-address adds `address`; wait-balance adds
`minimumMsats`; invoice exposes `invoice`, `operationId`, and `amountMsats`;
await-invoice exposes the terminal lower-snake-case `state` and resulting
balance; remit-guardian-fee exposes `federationId`, `operationId`, and
`amountMsats`. Receive addresses and invoices are intentionally returned to
the local caller; wallet root-secret bytes are never rendered.
