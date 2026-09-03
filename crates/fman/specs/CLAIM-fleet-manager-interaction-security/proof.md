# Proof: Fleet Manager remote-interaction confinement

## Status





## Property

This is an implementation-grounded conditional argument for
[CLAIM-fleet-manager-interaction-security](../CLAIM-fleet-manager-interaction-security.md).
The property concerns the effects of arbitrary remote bytes, identities, replay,
collusion, races, crashes, and concurrent verbs on the public RPC, Nostr, and
guardian-network surfaces. Each surface's contract below defines the authority
and effects it intentionally grants. Confinement requires both that an
interaction has no effect outside that contract and that it causes none of the
forbidden outcomes listed in the claim.


## Scope and model

The implementation scope is the FMan `bin`, `core`, `fedimint`, and `nostr`
crates; the Fleet Manager service wire crate; the shared operator authentication
adapter; [ARCH-fleet-manager](../ARCH-fleet-manager.md);
[SPEC-fi-rpc](../SPEC-fi-rpc.md);
[SPEC-signed-envelopes](../../../service-fleet-manager/specs/SPEC-signed-envelopes.md);
[SPEC-admin-socket](../SPEC-admin-socket.md);
[SPEC-operator-http](../SPEC-operator-http.md); and the FMan sections of the
root [SECURITY.md](../../../../SECURITY.md).

The input actors are an unauthenticated public RPC caller, an FI signer, a Nostr
relay and event publisher, federation peers and clients, and an arbitrary
guardian-protocol peer. The model permits arbitrary wire bytes, identities,
collusion between remote actors, replay, concurrent calls and verbs, dependency
errors, and a crash at any instruction boundary. Local filesystem and operator
access are not remote actors: they are trusted by the assumptions.

The protected outputs and effects are:

- RPC and operator responses and errors;
- Nostr advertisements, encrypted backups, and relay queries;
- logs and guardian-child stdout and stderr;
- guardian-child RPC, P2P, API, UI, and metrics traffic;
- payment-federation, guarded-federation, and Bitcoin-RPC operations;
- SQLite, RocksDB, and guardian-data filesystem mutations; and
- process creation, termination, and environment inheritance.

The protected assets are the mnemonic and derived keys, service and backup
signing keys, seat API credentials, bitcoind and payment-federation credentials,
bearer ecash, seat and fleet authority, admitted policy and trust state,
guardian-fee value and authority, and guardian data after invite exposure.

The contracts granted by the three remote surfaces are:

- **Public RPC:** the three unsigned verbs may read and report availability,
  validate a proposed purchase and return a signed stateless quote, or read and
  return signed public trust material. The ten signed verbs may verify and
  settle the signer's quote to create its seat, or read and operate only an
  existing seat owned by that signer according to the named FI verb.
- **Nostr:** remote relay data may update admitted setup-payment policy, may
  enter the durable Holder-authorization cache only during an operator-requested
  enrollment refresh, or may supply an operator-requested restore. FMan may publish only
  its public advertisement and encrypted recovery material. Relay acknowledgments
  and failures may affect publication progress and observed directory presence.
- **Guardian network:** a child may participate as its one seat's guardian,
  persist that guardian's state, contact its configured Bitcoin node and
  federation peers, and expose that guardian's Fedimint protocol. It has no
  fleet or other-seat authority. FMan may use the child's owner-local API for
  that seat's FI and operator verbs.

Availability, resource exhaustion, capacity, latency, operating cost, and
traffic analysis are not quantified by the property. A remote call may consume
the bounded work intentionally offered by its surface. The owner-only Admin
socket and authenticated operator HTTP adapter intentionally return secrets and
exercise fleet-wide authority for the honest operator; those outputs are not
public/Nostr/guardian disclosures. This proof nevertheless enumerates both
operator transports to check that they do not create an unaccounted public
channel.


## Assumptions

The proof grants each immediate premise from the
[claim record](../CLAIM-fleet-manager-interaction-security.md) exactly. It does
not inspect or establish the supporting analyses or any premise's evidence.
The following exact premises are numbered in their listed order for use below:

- Standard signature, hash, mint blinding, BIP-39, and labelled key-derivation
  schemes satisfy their stated security properties.
- The host is single-tenant; its filesystem permissions protect the data root;
  operator-controlled local processes and the owner-only Admin socket are
  trusted; and the pre-local-parameters localhost window of a new `fedimintd`
  is not adversarial.
- The pinned `fedimintd`, Fedimint client, iroh, SQLite, RocksDB, filesystem,
  operating-system process isolation, and Nostr dependencies satisfy their
  documented security contracts or fail detectably.
- The configured Bitcoin node has an honest chain view and uses credentials the
  operator is authorized to provide; the host clock meets protocol freshness
  bounds; committed SQLite writes survive crashes; and the data-root lock
  excludes another daemon using that root.
- The Admin caller is the honest operator, including when asserting that the
  original host is gone before restore.
- The pinned setup-payment publisher is uncompromised and publishes no malicious
  policy, and the chosen setup-payment federation retains an honest threshold.
- An FI-authenticated request obtains privileged access only to a seat whose
  durable owner is that request's verified signer; this imports only that part
  of
  [CLAIM-fleet-manager-fi-seat-access-owner-bound](../CLAIM-fleet-manager-fi-seat-access-owner-bound.md).
- [CLAIM-fleet-manager-confines-seat-local-authority](../CLAIM-fleet-manager-confines-seat-local-authority.md)
- [CLAIM-fleet-manager-confines-secret-dependent-content](../CLAIM-fleet-manager-confines-secret-dependent-content.md)
- Fleet Manager creates a paid seat only after verifying its exact quote-bound
  payment and value; this imports only that part of
  [CLAIM-fleet-manager-paid-seat-payment-verified](../CLAIM-fleet-manager-paid-seat-payment-verified.md).
- [CLAIM-fleet-manager-quote-settlement-exclusive](../CLAIM-fleet-manager-quote-settlement-exclusive.md)
- [CLAIM-fleet-manager-preserves-published-guardian-data](../CLAIM-fleet-manager-preserves-published-guardian-data.md)
- Only the current, complete, valid publication from the pinned publisher
  determines admitted setup-payment policy; relay withholding or replay cannot
  select an older authentic policy.
- Only holder-authentic authorizations bound to this Fleet Manager enter its
  served trust material. After this daemon version normalizes and exposes an
  in-bound authorization, relay withholding cannot erase every enrolled row
  while the receiver maximum issue time does not move backward; replay cannot replace an
  enrolled same-credential authorization with an equal or older one. Relying
  FIs check current issuer policy and revocation.
- Restore adopts only current, authentic, internally consistent documents bound
  to the recovered identity; relay withholding or replay cannot select older
  authentic recovery state.
- The bundled, pinned `fedimintd` and its dependencies retain intended
  implementation integrity under hostile network input. This is an explicit FMan
  TCB premise, not malicious-child containment.
- Guardian-fee collection and sweeping use only authorized, attributable
  effects. Collection output counts only terminally confirmed value, preserves a
  structured incomplete outcome after any durable operation exists, and exposes
  no dependency error text in that successful operator response.
- Production cannot select development trust roots, placeholder identities, or
  development-only trust overrides.
- Each guarded federation retains an honest threshold, and its client API
  satisfies the protocol semantics Fleet Manager uses for guardian-fee
  inspection, collection, and withdrawal.

Among premises 7–13, premises 8, 9, 11, and 12 grant complete linked-claim
properties; premises 7 and 10 grant only the qualifications stated in this
list; and premise 13 is a direct premise supported by a supporting analysis. Premises 14–20 deliberately bottom out policy, trust, restore, child
confinement, guardian fees, environment selection, and guarded-federation
behavior at this claim boundary.


## Argument

1. **[code] The public RPC inventory is complete.**
   [`FleetManagerService`](../../../service-fleet-manager/src/service.rs)
   contains thirteen methods. Three are intentionally unsigned:
   `get_availability`, `get_quote`, and
   `get_fman_trust_material`. Ten take `SignedRequest`:
   `create_seat`, `get_dkg_code`, `start_dkg`, `restart_dkg`, `get_status`,
   `get_invite_code`, `get_peer_attestation`, `set_meta_field`,
   `propose_guardian_fees`, and `get_fedimint_stats`. The production
   [`FleetManagerService` implementation](../../core/src/service.rs) implements
   exactly this trait, and
   [`main`](../../bin/src/main.rs) installs that implementation in the sole
   `FLEET_MANAGER_ALPN` router. Regenerating the trait methods and its
   implementation yields the same 3+10 partition.

2. **[code] Signed FI authority enters through one boundary.**
   Each of the ten signed methods first calls `FleetManagerRpc::validate`.
   Verification authenticates the exact received payload bytes under a
   direction- and verb-specific label, parses only after signature validation,
   requires inner and outer FI identities to match, and enforces the clock
   window. Nine target an existing seat and make `Fleet::authorize` their first
   fleet selection. `get_dkg_code`, `start_dkg`, and `restart_dkg` enqueue that
   seat's local-parameter, DKG, persistence, child, and backup effects.
   `get_status`, `get_invite_code`, and `get_peer_attestation` read that seat
   and return its status, invite, or signed binding. `set_meta_field` and
   `propose_guardian_fees` submit validated metadata through that seat's local
   child API. `get_fedimint_stats` authorizes and then returns
   `UnsupportedVerb`, producing no seat effect. The remaining signed method,
   `create_seat`, verifies the FMan-signed quote, quote coherence, and quote FI
   against the verified signer before presenting a verified payment to
   allocation, whose allowed contract is one durable acceptance or refusal and
   the corresponding seat and claim effects or signed refund-transaction
   output. Premises 1, 4, and 7–13 therefore bound forgery, seat authority,
   cross-seat effects, payment, settlement, deletion, and operator value.

   The observed effects do not define the contract. The governing
   [FI RPC contract](../SPEC-fi-rpc.md) and its boundary checks constrain every
   grant: `create_seat` requires a coherent manager-signed quote bound to the
   verified FI and exact verified payment; `get_dkg_code` bounds display names,
   rejects module configuration, and preserves recorded local parameters;
   `start_dkg` accepts only a valid code set in the formation lifecycle;
   `restart_dkg` is refused after consensus is durably observed and otherwise
   changes only the current formation attempt; status, invite, and attestation
   are read-only projections of the authorized seat, with invite and binding
   available only in their valid lifecycle states; `set_meta_field` accepts
   only a compiled [`fman_meta_fields`](../../meta-fields/src/lib.rs) key and its
   typed value domain; `propose_guardian_fees` bounds the rate and recipient
   list and requires the canonical proposal to pay this seat's derived account;
   and stats is unsupported. [`SeatLoop`](../../core/src/seat.rs) enforces each
   lifecycle gate in the same serialized command that performs the child or
   persistence effect. Thus the source roster cannot silently redefine a
   broader effect as contract-granted.

3. **[code] The three public verbs stay within their read and quote contracts.**
   `get_availability` calls `Fleet::availability_snapshot`, which reads the
   transactional offer projection and returns only the supported-version,
   supported-size, current offer, and capacity projection granted by
   `SPEC-fi-rpc`. `get_quote` first requires the supported version and size, an
   exactly offered plan, an admitted payment federation for nonzero price, and
   coherent refund preparation. It reads one transactional `quote_offer`
   snapshot, calls
   `wallet.quote_locked` and `wallet.validate_quote_refund`, and returns the
   contract's stateless signed quote. It writes neither a seat nor a
   settlement; premise 13 covers operator-value consumption by these
   unauthenticated wallet calls.
   `get_fman_trust_material` accepts only the protocol version, reads the
   current public API URL and premise-15 FMan-wide holder-authorization set,
   and signs the bounded response under the advertised FMan identity. It does
   not inspect fleet, seat, child, or federation state; consensus
   `fedi:fman_seat_bindings` is the separate authority for seat membership.
   Premises 7–13 and 17 exclude authority or protected effects through the
   wallet and this public identity read.

4. **[code] The Nostr input inventory is complete at this boundary.**
   [`fman-nostr`](../../nostr/src/lib.rs) admits two relay-derived categories
   during normal operation: setup-payment policy publications and holder
   authorizations. [`fman-nostr::backup`](../../nostr/src/backup.rs) supplies
   the third category, encrypted backup documents and guardian archives, during
   onboarding restore. Other Nostr operations publish advertisements or
   backups, confirm publication, or affect liveness only. Premises 14, 15, and
   16 respectively grant current complete valid policy, authorization, and
   restore inputs; premises 5 and 6 grant the operator and publisher decisions.
   A relay replay, omission, reordering, duplicate, or injected event cannot
   create another admitted state while those premises hold.

5. **[code] The Admin inventory is complete and remains a trusted boundary.**
   [`AdminRequest`](../../core/src/admin.rs) has seventeen variants:
   `ShowPlans`, `SetPrice`, `ListPaymentFederations`, `PayoutDestination`,
   `SetPayoutDestination`, `SweepPaymentFees`, `ListSeats`, `SeatStatus`,
   `DecommissionSeat`, `GuardianFees`,
   `CollectGuardianFees`, `SweepGuardianFees`, `Onboarding`,
   `RefreshHolderAuthorizations`, `ShowMnemonic`, `OnboardAsNew`, and
   `OnboardFromBackup`. The Unix transport
   binds an owner-only socket under the locked data root. The optional
   [`admin_http`](../../core/src/admin_http.rs) adapter routes the same enum to
   the same dispatcher behind a whole-router authentication layer; password
   mode checks a session cookie, while trusted-proxy mode deliberately assigns
   authentication to its sole deployment peer. `main` disables the listener
   unless bind and auth mode agree and validates the password-file boundary.
   Under premises 2 and 5, only the honest operator reaches the dispatcher.
   Consequently the mnemonic, OOB tokens, restore phrase, fleet-wide reads, and
   fleet-wide mutations on these two transports are intentional operator
   effects rather than remote-surface disclosures.

6. **[code] Guardian and external effects have no omitted authority source.**
   The complete handler/producer-to-effect roster is:

   - **Public RPC:** `get_availability` reads the Fleet/SQLite projection;
     `get_quote` reads `quote_offer` and calls `quote_locked` and
     `validate_quote_refund`;
     `get_fman_trust_material` samples the public endpoint and durably enrolled
     FMan-wide holder authorizations, then signs and validates one bounded
     identity response without fleet, seat, child, or federation access. These
     are the three paths detailed in
     [`service`](../../core/src/service.rs) and lemma 3.
   - **Signed RPC:** `create_seat` calls `verify_locked`, then
     [`Fleet::create_seat`](../../core/src/fleet.rs), whose outcomes write
     SQLite, claim accepted payment through the payment client, return the
     already prepared signed refund transaction on refusal without submitting
     it, start a child for an accepted seat, and enqueue a backup.
     `get_dkg_code`, `start_dkg`, and `restart_dkg` call the corresponding
     [`SeatLoop`](../../core/src/seat.rs) commands: they use the localhost child
     API, write DKG facts to SQLite, enqueue backups, and may start/stop the
     child; `restart_dkg` also removes that seat's guardian directory before
     respawn. `get_status`, `get_invite_code`, and `get_peer_attestation`
     serialize child probes/reads and return data only. `set_meta_field` and
     `propose_guardian_fees` submit validated metadata to the child API.
     `get_fedimint_stats` has no post-authorization effect.
   - **Running Admin:** in [`admin::dispatch`](../../core/src/admin.rs),
     `ShowPlans`, `ListSeats`, `Onboarding`, and `ShowMnemonic` are local reads;
     `RefreshHolderAuthorizations` schedules the Nostr enrollment task;
     `SetPrice` performs the SQLite offer/epoch transaction;
     `ListPaymentFederations` calls payment-wallet status/receivability reads;
     `SweepPaymentFees` calls the payment wallet's Lightning payout path; `SeatStatus`
     performs the serialized seat report plus guarded-federation fee reads;
     `DecommissionSeat` writes the terminal SQLite fact, stops the child, and
     enqueues its backup; `GuardianFees` performs guarded-federation status,
     policy, remittance, and wallet-balance reads; `CollectGuardianFees` performs
     one or two guarded-federation operations and returns either the unchanged
     complete shape or structured incomplete progress after a durable operation;
     `SweepGuardianFees` performs the collected-ecash Lightning payout.
     Both payout implementations start through the pinned client's native
     v1/v2 durable operation commit. Their exact-id status/await paths first
     validate the operation's FMan payout metadata in the selected wallet scope
     and then read or subscribe to that operation; neither path reaches LNURL
     invoice acquisition or either rail's payment-start call. The shared
     projection reports rail state separately from the active operation set, so
     rail-terminal change/refund work cannot be mistaken for fully quiescent
     wallet state.
     Running-fleet `OnboardAsNew` is a read/idempotent answer or error,
     and `OnboardFromBackup` is refused.
   - **Onboarding Admin:** before a Fleet exists,
     [`onboarding::Onboarding`](../../core/src/onboarding.rs) accepts only
     `OnboardAsNew` and `OnboardFromBackup`. The former writes the new identity
     to SQLite. The latter queries the Nostr backup archive, authenticates and
     reconstructs the documents under premise 16, writes recovered guardian
     directories and SQLite state, and then writes the recovered identity.
     Other Admin variants are refused.
    - **Nostr/background:** [`FleetManagerNostr`](../../nostr/src/lib.rs)
      queries Holder authorizations once when its runtime starts and after each
      later Admin refresh request, durably merges verified complete events
      through `FleetHolderAuthorizationStore`,
     periodically queries setup-payment publications, publishes the Fleet
     advertisement, updates directory presence, and writes admitted policy
     through `FleetSetupPaymentPolicyStore`; the setup-payment
     reconciler may consequently join a payment client in RocksDB.
     `backup_queue` reads SQLite and guardian
     directories and asks the [`Nostr backup
     sink`](../../nostr/src/backup.rs) to publish encrypted archives/documents
     and confirm them. Payment-outcome recovery retries only the claim operation
     already assigned to `create_seat`.
   - **Child/guardian:** `supervisor::spawn_child`
     creates the guardian directory and one child with distinct per-seat API
     and P2P Iroh identities, four localhost binds (P2P, API, UI, and metrics),
     explicit Bitcoin credentials, a cleared environment, and piped stdout and
     stderr. The supervisor's initial-wipe path and FI `restart_dkg` in
     `SeatLoop` are the two live guardian-directory deletion sites. Output
     pumps emit bounded tracing records. The child originates Bitcoin RPC and
     its one seat's public guardian protocol; `SeatLoop` alone possesses its
     localhost API credential. [`fman-fedimint`](../../fedimint/src/lib.rs)
     owns the separate RocksDB payment- and guarded-federation client scopes.
   - **Outputs:** [`service`](../../core/src/service.rs),
     [`admin`](../../core/src/admin.rs), and
     [`admin_http`](../../core/src/admin_http.rs) are the RPC, Unix, and HTTP
     response/error channels. Internal RPC errors are sanitized; Admin
     responses intentionally contain operator data. The Nostr and child
     producers above are the other network and log outputs; dependency,
     child-output, retry, and runtime failures reach tracing.

   Premises 2–4 grant local custody, dependency contracts, process isolation,
   failure detection, chain view, persistence, and the single-root process
   model. Premises 8, 9, 17, 18, and 20 grant cross-seat isolation, output
   confinement, bundled-child implementation integrity, authorized fee effects, and
   guarded-federation semantics. Every rostered effect is therefore either the
   named surface contract, an honest-operator effect, or an internal
   consequence confined by an immediate premise. No adversarial
   public/Nostr/guardian path reaches the Admin dispatcher, another seat's
   loopback API credential, another wallet client scope, or a process-spawn
   parameter.

7. **[code] Replay and concurrency add no authority.**
   Unsigned read replay repeats only the effects assigned in lemma 3. Signed
   replay must retain the same verified signer and verb label. `create_seat`
   replay is covered by the durable single-outcome premises 10–13; an
   existing-seat replay remains confined to the same owner and seat by premises
   7 and 8. A seat's command loop serializes concurrent lifecycle verbs, and
   allocation and offer changes share SQLite's immediate writer boundary.
   Concurrent Admin activity is activity of the honest operator.
   Thus an interleaving can change which valid state a request observes or make
   it fail, but cannot supply a new signer, seat, policy source, secret channel,
   or payment/fee authority.

8. **[code] Crash boundaries do not create a new accepted effect.**
   A crash can interrupt an RPC response, Nostr publication, child operation,
   payment action, or local write. Premises 3 and 4 grant detectable dependency
   failure and committed-write durability. Premises 10–13 grant the relevant
   payment, outcome, value, and guardian-data predicates across every such
   interruption; premises 14–16 grant current admitted state after restart.
   The data-root lock and child process-isolation premises exclude a concurrent
   second daemon taking over the same local authority. Crash-only loss of
   availability remains outside the property.

9. **[assumption] The contracts and assumptions cover the complete conclusion.**
   Premise 9 excludes the listed secret disclosures; 7, 8, and 17 exclude
   cross-seat and fleet authority; 10 excludes unpaid allocation; 11 excludes
   double settlement; 13 excludes unrelated operator-value consumption; 14–16
   exclude attacker-selected policy, trust, and restore state; 18 and 20
   exclude guardian-authority misuse; and 12 excludes post-invite deletion.
   Premise 19 prevents production from substituting development trust material.
   Lemmas 1 and 4–6 account for every input, output, persistence, process, and
   external-effect channel; each rostered effect is assigned to the public RPC,
   signed FI, Nostr, guardian, or trusted Admin contract. Lemmas 2, 3, 7, and 8
   close authentication, replay, race, and crash transitions, so no
   interaction can acquire an unassigned effect. Therefore every interaction
   remains inside its surface contract and no listed forbidden effect remains
   possible when all immediate assumptions hold.


## Residuals

- Denial of service, resource exhaustion, availability, capacity, latency,
  port exhaustion, and operating cost are outside the stated protected effects.
- A hostile host, local process, operator, trusted proxy, setup-payment
  publisher, Bitcoin node, dependency contract, payment-federation threshold,
  or guarded-federation threshold violates an immediate premise rather than the
  local implication.
- Operator responses intentionally contain sensitive values. Their
  confidentiality after they leave FMan, browser transport security, password
  quality, and authenticating-proxy configuration belong to the trusted
  operator/deployment boundary, not the public/Nostr/guardian adversary.
- Running the same mnemonic from another host or data root is excluded by the
  honest restore assertion and single-root deployment premises.


## Weakest links

The public, Admin, Nostr, and external-effect inventories are source-read
`code` lemmas rather than mechanically pinned tests. The exact method, variant,
router, and effect lists must be reread when those sources change. The
guardian-child confinement, current relay-derived input, and guardian-fee
premises intentionally carry most of the difficult security argument; this
proof does not add confidence below those axioms. The operator HTTP
trusted-proxy mode is deployment-enforced rather than locally authenticated,
and the public trust-material endpoint has no endpoint-specific rate limit;
both are outside this confidentiality/authority property only under the stated
model and residuals.



## Additional current evidence

# Evidence: relay withholding silently empties trust material




Scope: `crates/fman/nostr/src/lib.rs`,
`crates/fman/bin/src/main.rs`,
`crates/fman/core/src/{db,fleet,service}.rs`,
`crates/fman/core/migrations/**`,
`crates/fman/core/tests/db.rs`,
`crates/fman/specs/SPEC-fi-rpc.md`,
`crates/domain/src/fman_federation_directory.rs`, and
the production-readiness fault model

## Claim

Under V1's fault model in the production-readiness fault model, after this version of the daemon has
normalized, learned, and exposed at least one Holder-authorization envelope,
outage or withholding by every configured Nostr relay cannot make the otherwise
local `GetFmanTrustMaterial` status/report verb silently change its
authorization vector from nonempty to empty (A2), provided the receiver's
maximum admissible issue time does not move backward.

The relay may omit or delay all events, selectively return authentic
replacements, and then recover. The claim permits a greater-`issued_at`
authorization for the same credential digest to replace the exact earlier
envelope; it asserts retained nonemptiness, not append-only history or live
credential validity. The operator restarts the daemon at most once.

## Axioms (trusted, not checked here)

- **A-host/deps:** V1's A-host and A-deps-recover hold. The same data root and
  mnemonic are used after restart.
- **A-sqlite:** committed SQLite transactions survive the admitted crash and
  reopen with their complete rows; parameter binding and row reads preserve the
  stored BLOB and TEXT values; database corruption and manual writes do not
  occur.
- **A-authorization:** before the outage, at least one authentic Holder
  authorization for this FMan was fetched, normalized by this version's
  bounded loader, and exposed by `GetFmanTrustMaterial`.
- **A-clock:** from first exposure through the quantified refreshes and optional
  restart, each `now + 1h` maximum passed to the durable store's `merge` or
  `load` operation is greater than or equal to the preceding durable-operation
  maximum. Checked addition succeeds. This premise constrains the receiver
  clock, not the authorization vector or retained rows; the fetch-only verifier
  maximum is irrelevant to destructive normalization.
- **A-client:** an FI can still reach the FMan's Iroh RPC endpoint during the
  relay outage. This is the A2 isolation condition, not an added availability
  dependency.

## Argument

**L1 (code + schema + test) — first exposure implies at least one durable row
was committed first.** The initial or an operator-triggered refresh verifies
each relay candidate before constructing the retention input. A nonempty refresh awaits
`FleetHolderAuthorizationStore::merge` before reloading the retained events and
replacing the runtime authorization watch. The migration makes credential
digest a constrained primary key. The named test
`holder_authorization_events_merge_monotonically_and_survive_restart` checks
that multiple entries survive reopen, an older same-digest event cannot replace
a newer one, and an empty merge does not erase in-bound rows. The exact-boundary
tests separately pin the receiver-time and 64-row aggregate limits. Because
A-authorization observed a nonempty RPC vector after normalization, at least
one bounded row's transaction completed first.

**L2 (enum + code + schema) — every refresh preserves one in-bound row.** The
authorization worker runs once when the runtime starts, then after each
explicit Admin notification. A fetch error performs no store or watch write. An empty candidate vector performs no
merge but still reloads normalized state. A nonempty result enters one immediate
transaction that removes future-issued legacy rows, inserts a new digest only
below the 64-row limit, and updates an existing digest only for a greater
in-bound signed `issued_at`. At first exposure every row is within that
receiver maximum. A-clock keeps each old row and every subsequently admitted
same-digest successor within later maxima. No insert evicts an existing valid
row, and an update keeps the same primary-key row, so failure rolls the
transaction back and success leaves at least one row.

After a successful merge, reload reparses and reverifies every complete event
before replacing the watch. Every stored event entered through that same
verification path, so value preservation under A-sqlite means reload yields at
least one envelope. Thus incomplete prefixes and equal/older replay cannot
empty state, while an authentic greater same-digest replacement keeps it
nonempty.

**L3 (code + axiom) — restart reconstructs in-bound retained state before
binding the trust source.** Startup opens the same SQLite data root, removes
future-issued rows and any pre-fix rows beyond the deterministic aggregate
bound, then loads and reverifies every remaining complete event. Because
A-authorization starts after this version already normalized and exposed the
set, aggregate cleanup cannot newly select away its rows; A-clock keeps at least
one row through time cleanup. The public router is already spawned, but its
trust-material source is an unbound fail-closed slot; pre-binding requests
return an error. Only after the load succeeds does startup seed
`FleetManagerNostr` and bind the concrete source. A-sqlite preserves the
designated event, so the admitted restart does not bind an empty authorization
source.

**L4 (enum + code) — the reachable FI response remains nonempty.** The
official `NostrTrustMaterialSource` clones the seeded or post-refresh
authorization watch. `get_fman_trust_material` places that vector into
the signed response without consulting the relay. L1 establishes durable
nonemptiness, L2 preserves it across every refresh, and L3 preserves it across
restart; therefore the reachable FI response cannot silently become empty due
to withholding. ∎

## Residual windows

- Before any authorization has been successfully enrolled, relay withholding
  can keep the cache empty. That is discovery unavailability, not the
  nonempty-to-empty transition quantified here.
- An upgrade can delete pre-fix future-issued or over-cap rows before this
  version first exposes its normalized set. A later backward wall-clock step can
  also move every retained row beyond the receiver-time bound. A-authorization
  and A-clock exclude both cases; they are local admission cleanup rather than
  relay withholding.
- A greater-`issued_at`, same-digest authentic event replaces the prior exact
  envelope. This is the intended credential-authorization replacement rule,
  not relay omission; this claim preserves nonemptiness rather than every
  historical byte.
- A mnemonic-only restore without the original SQLite data root does not carry
  this local cache. A-host/deps explicitly requires the same data root; the
  operator must refresh enrollment after such a restore.
- Retention does not implement Holder retraction, authorization expiry, or
  credential revocation. Relying FIs remain responsible for fresh issuer-policy
  and revocation checks.
- Database corruption, broken durability, and manual writes are excluded by
  A-sqlite. Invalid retained bytes fail startup rather than binding an
  authoritative empty source.

## Weakest links

L2 is a scoped writer, destructive-update, deletion, and migration enumeration
and must be regenerated for any store or migration change. L3 relies on the
binary's fail-closed late binding, same-data-root premise, and A-clock's
wall-clock bound. The focused database tests establish clean reopen, aggregate
and future-time cleanup, and merge mechanics, not crash durability or a live
relay outage through the RPC exit channel.

# Evidence: trust material cannot redefine federation state




Scope: `crates/fman/core/src/{directory,fleet,seat,service}.rs`,
`crates/fman/core/tests/{fleet,service}.rs`,
`crates/domain/src/fman_federation_directory.rs`,
`crates/fman/specs/SPEC-fi-rpc.md`, and
the production-readiness proof

Imports:

- [CLAIM-fleet-manager-holder-authorization-bound](../CLAIM-fleet-manager-holder-authorization-bound.md):
  every envelope the official daemon admits, reports, advertises, or sends in
  telemetry has a Holder proof verifying under its statement Holder key, a
  statement subject equal to this FMan's Nostr service key, and a signed
  credential payload whose digest equals the statement credential digest.

## Claim

Under the fault model of `wip-fman-has-no-bugs`, an honest-path
`GetPeerAttestation` binds the requested owned seat to that seat's own durable
final federation config and peer identity. An honest-path
`GetFmanTrustMaterial` response makes no federation or seat claim: it binds the
current public endpoint and admitted holder-authorization set to this FMan's
service Nostr key (H4), with a bounded freshness interval. Consensus
`fedi:fman_seat_bindings` separately determines where that identity operates.

Holder-authorization *authenticity and admission* are imported from
[CLAIM-fleet-manager-holder-authorization-bound](../CLAIM-fleet-manager-holder-authorization-bound.md); this claim adds only the
honest-path carriage and restart analysis. It does not claim that the returned
authorization vector is complete or currently valid under issuer policy.

## Axioms (trusted, not checked here)

- **A-root/deps:** the root's A-host, A-deps, and honest-counterparty axiom
  hold. A running fedimintd's final client config and own invite code faithfully
  represent its durably installed guardian config.
- **A-response-domain:** the shared federation derivation, digest, response
  signing, and response verification functions implement their documented
  encodings and signature semantics. The imported claim supplies the Holder
  authorization signature and hash premises used below.

## Argument

**L1 (code + test) — one peer attestation is derived from one seat's own final
config.** The signed FI request is authorized to its recorded seat before the
seat loop runs `federation_binding`. That operation obtains a running client
and final config together, derives the federation roster and config hash from
that config, obtains the guardian's own peer id from its invite code, and
selects exactly that peer's roster entry. Pre-consensus, decommissioned,
unreachable, unparsable, or internally inconsistent seats fail instead of
being attested. `sign_peer_attestation` copies every binding field and signs
its digest. The fleet tests pin pre-consensus refusal and the service tests pin
request ownership.

**L2 (enum + code) — the live identity response cannot redefine federation
membership.** `GetFmanTrustMaterialRequest` contains only `ProtocolV1` and
rejects unknown fields. The response contains no federation id, config hash,
peer id, or peer attestation. Its canonical digest covers the FMan pubkey,
current public URLs, freshness interval, and authorization envelopes. The
shared `verify_for_fman` entry point requires an expected pubkey, then verifies
that exact identity, the signature, bounds, URL canonicality, freshness, and
authorization subjects. Callers obtain the expected identity from the verified
consensus directory rather than from this response.

**L3 (claim + code) — official holder-authorization carriage cannot create a
seat or federation binding.** The official trust source clones the runtime's
durably enrolled envelope vector, whose three binding properties are the
imported claim conclusion. Those global FMan authorizations do not name a
federation. The response validator requires every authorization subject to
equal the signing FMan identity, and the outer signature covers the complete
bounded vector.

**L4 (code + claim + axiom) — concurrency and restart affect freshness, not
membership.** The handler reads no seat or child state, so concurrent
formation, decommission, or child failure cannot alter or delay its membership
answer: it has none. A daemon crash emits no response. After restart, the
authorization watch is seeded from complete cached events only after they are
reverified; an invalid cache fails startup rather than serving it. The router
exists before cache loading, but pre-binding trust requests return `Unsupported`
rather than an empty document. Every item returned after binding still
satisfies the imported conclusion, while relying verifiers independently join
the response to consensus membership and perform issuer-policy and revocation
checks.

**Conclusion.** L1 prevents per-seat misbinding in the separate authenticated
attestation API. L2 makes the public trust response incapable of asserting seat
membership and binds it to the exact consensus-selected FMan identity. L3 and
L4 preserve authorization authenticity across carriage and restart. Under the
axioms, the live response cannot falsely place an FMan in a requested seat or
federation. ∎

## Residual windows

- Holder authorizations are durably cached and reverified before the runtime
  source is bound after restart. Relay omission can still prevent enrollment of
  an unseen or replacement authorization, and the response has no completeness
  marker. Relying FIs must perform fresh issuer-policy and revocation checks;
  any stronger live-completeness/current-validity claim is false.
- The live response proves a recent FMan identity snapshot, not that the FMan is
  currently serving any particular federation. That membership conclusion
  requires the separately verified consensus seat-binding directory.
- Imported admission proves narrow authorization authenticity, not issuer
  validity, credential trust, current holder intent, or revocation. Those
  limitations remain the imported record's residuals.

## Weakest links

L1's authenticated seat path and the public sink enumeration remain manual.
A-root/deps carries the correspondence between fedimintd's live APIs and its
durable final config. The live response deliberately makes no federation
membership claim; correctness of the relying party's consensus-directory join
is outside this FMan-local subclaim.
