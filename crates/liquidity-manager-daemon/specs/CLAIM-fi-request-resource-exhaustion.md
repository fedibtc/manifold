# CLAIM-fi-request-resource-exhaustion: Fi request resource exhaustion

For the official FLIP daemon and its one SQLite data root, no FI-controlled
family can cause the number of durable `allocations` rows attributable to that
family to grow without bound unless every increase first spends a
**durable-state admission token**. “Without bound” means without a finite
cardinality or retention bound: for every `K` that fits the finite host storage,
the family must not be able to reach a committed state containing at least `K`
of its retained rows without those token expenditures.

The admission token is a valid, unrevoked FMan endorsement for the exact
federation the row is keyed by. `allocations` has `federation_id` as its primary
key and admission requires such an endorsement, so one endorsement admits at
most one row. A5 states what bounds their supply.

The adversary controls any number of self-generated FI/Sybil keypairs, valid
signatures under those keys, FI-provided request fields, replay, concurrency,
timing, and disconnects. Ordinary daemon crashes, restarts, dependency delay,
and background-task interleavings are in scope. Authenticated Admin/operator
actions, setup, configured trust roots, wallet funding, direct database edits,
backup/restore, out-of-band wallet activity, and malicious configuration
changes are not adversarial verbs.

## Status

Unverified.

## Assumptions

- **A1 — storage/process semantics.** SQLite/SQLx commits and uniqueness
  constraints behave as declared; a committed row survives an ordinary daemon
  crash/restart; uncommitted transactions do not. The official daemon is the
  only writer, and the host supplies enough storage for the finite `K` chosen
  in a trace. This grounds the durable predicate without pretending physical
  storage is infinite.
- **A2 — cryptography and canonical encoding.** A party controlling a valid
  Nostr/Schnorr secret key can serialize the public request type, compute its
  canonical payload and details hashes, and produce a signature accepted for
  the corresponding canonical requester pubkey. SHA-256, Schnorr verification,
  serde, and canonical CBOR have their library-defined behavior. No forgery is
  required.
- **A3 — trusted provider preconditions.** A trusted operator has installed the
  provider identity and ordinary setup needed for the daemon to run its public
  Iroh endpoint, configured a bind address reachable by the FI, and made its
  endpoint identity and provider pubkey known out of band. Non-fixture mode
  publishes no ready advertisement and the default bind is loopback, so this
  reachability/knowledge premise is explicit rather than inferred from
  discovery. These are trusted preconditions.
- **A4 — current dependency wiring.** The production trust pipeline runs
  against a real invite-code preview, the FMan trust material the request
  carries, and real revocation relays; `TrustInputs::Production` reports
  `inputs_available: true`. `--trust-fixtures` substitutes the invite-code
  preview only, leaving advertisements and revocations real, and reports inputs
  available as well; fixture-backed configuration is refused on Bitcoin mainnet.
  There is no separate FMan trust-material transport provider, because material
  is request-carried.

  **This is not an issuer-side quota.** Issuers do not issue endorsements to federations. An issuer badges
  an FMan *identity*; the endorsement is an attestation signed by that FMan
  naming any federation id it chooses, and the code's own comment records that
  the attestation is self-signable. The gate checks signature, federation-versus-
  invite match, installed issuer, badge-to-signer binding, and revocation —
  none of which caps how many distinct federation ids one unrevoked badge can
  name. So "bounded endorsement supply" was the wrong scarce resource; what the
  claim actually needs is a bound on admissible *federations*.

  The premise is stated rather than proved because **FLIP cannot establish it**.
  Whether badges are issued freely, and whether an adversary can stand up
  arbitrarily many previewable federations operated by badge-holding FMans, are
  properties of the trust configuration and of the wider system, not of this
  binary. FLIP's part is only that it admits no row without passing the gate.
  The
  [release envelope](../../../docs/liquidity-manager/liquidity-manager-release-envelope.md)
  records the same thing from the other side: FLIP's real workload ceiling is
  set by the badge policy of the issuers an operator installs.
