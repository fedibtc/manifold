# ARCH-fleet-manager-identity: Mnemonic-rooted identity

The install's entire identity is a 12-word BIP-39 English mnemonic,
generated at first run and stored plaintext in SQLite. Every key derives
from its 64-byte seed (empty passphrase) via HKDF-SHA256 with versioned,
purpose-separated `info` labels:

- `fman/v1/service-nostr` → secp256k1 x-only key: the FMan's public
  identity — signs Nostr ads and peer attestations, and is the subject of
  holder authorizations. Its public key also deterministically produces the
  FMan's two-word presentation name; the public derivation is owned by
   [ARCH-service-fleet-manager](../../service-fleet-manager/specs/ARCH-service-fleet-manager.md),
  not by this secret-key tree.
- `fman/v1/service-sign` → commitment-response signing key; its pubkey is
  the locator `service_pubkey` FIs verify against (see *Signature scheme*
  below).
- `fman/v1/iroh` → Ed25519 iroh endpoint key (iroh NodeIds are Ed25519 by
  protocol construction; not a signing identity).
- `fman/v1/payment-wallet` → 64-byte root secret of the payment clients; every
  per-federation wallet key and locked-quote note key derives from it, so
  wallet funds are recoverable from the mnemonic alone
  ([SPEC-locked-payment](./SPEC-locked-payment.md)); only ever used locally,
  never a wire identity.
- `fman/v1/guardian-fee` → 64-byte root secret of the guardian-fee clients,
  then `fman/guardian-fee-client/seat/v1/<seat-id>` separates each seat before
  Fedimint derives its federation and module keys. Collected fee ecash therefore
  recovers from the mnemonic, invite code, and seat id selecting the client's
  scope.

  The separate label is load-bearing, not tidiness. An FMan that both guards a
  federation and accepts payments in it opens two client databases for that one
   federation; fedimint derives mint note secrets sequentially from the client
   root, so a shared root would hand both databases the same secrets and the
   same indices — colliding issuance and unrecoverable notes. The seat tweak
   applies the same separation to two guarded seats in one federation. It also
   keeps the two note pools separately accounted, so sweeping collected guardian
   fees cannot spend payment ecash
  ([SPEC-admin-socket](./SPEC-admin-socket.md) `SweepGuardianFees`).

   This separation belongs here rather than in a Fedimint root salt inside the
   wallet crate: this label list is the one place that answers what derives from
   what, and a salt hidden at the client boundary is invisible to it. The repository's pre-production persisted-format
   policy permits this per-seat derivation boundary to change
   without a migration. Guardian client ecash created before this boundary is
   deliberately outside the new root's recovery domain; development installs
   with such state must retain their old wallet database or drain it before
   adopting this pre-production format.
- `fman/v1/nostr-backup`, `fman/v1/nostr-backup-tag`,
  `fman/v1/nostr-backup-encryption` → the backup identity
  ([SPEC-nostr-backup-restore](./SPEC-nostr-backup-restore.md)): the Nostr
  keypair that authors backup events, the HMAC key that blinds their
  addressable coordinates, and the XChaCha20-Poly1305 key that seals their
  contents. Deliberately not the service keys — discovery and trust surfaces
  must not resolve recovery material — and three labels because the three
  jobs have different exposure: the author's pubkey and the coordinates are
  public on the relay, the sealing key never leaves the daemon.
- `fman/v1/seat/<seat-id>/guardian-fee-account` → secp256k1 key of that seat's
  `BtcDepositor` remittance account. Scoped to the seat and nothing else:
  deliberately not the federation id and not the stability-pool module's own
  derivation from the client root, both of which only exist after DKG, so the
  seat can state where it will be paid before its federation exists
  ([SPEC-guardian-fee-policy](./SPEC-guardian-fee-policy.md)). Fee revenue
  therefore recovers from the mnemonic plus the seat id.
- `fman/v1/telemetry/<generation>` → one 32-byte bearer capability for the
  FMan's seat discovery, raw Prometheus proxies, and safe-event journals.
  SQLite stores only the global monotone generation, so capability plaintext
  remains mnemonic-derived; re-enrollment advances the generation and revokes
  the previous bearer across every telemetry surface
  ([SPEC-guardian-telemetry-proxy](./SPEC-guardian-telemetry-proxy.md)).
- Quote-note spend secrets and blinding keys derive from the wallet root
  and the quote's 32-byte random nonce — deliberately **outside any
  mint client's sequential index tree**, because stateless quotes would
  otherwise burn indices and free quote spam could blow past the mintv1
  recovery gap limit. On mintv2 keys instead follow fedimint's own
  root+tweak derivation with plain random tweaks — deliberately not
  ground against the scan filter, so stock recovery never imports
  escrow-phase notes ([SPEC-locked-payment](./SPEC-locked-payment.md),
  *Recovery and escrow*). On either generation unclaimed quote notes
  are recoverable with the database or the FI's replay, until the
  background claim moves their value into the client's own tree.
- `fman/v1/seat/<seat_id>/iroh-api`, `…/iroh-p2p`, `…/api-auth` → the seat's
  two iroh endpoint keys and permanent fedimintd admin password. The iroh keys
  are handed to fedimintd as
  the driven-DKG `RunDkg` frame so seat NodeIds survive daemon restarts; formed configs retain the keys themselves.
  Derivation is flat (one HKDF level from the root seed); the daemon hands
  spawning layers only one seat's derived keys, never root material. (An
  earlier two-level scheme with a per-seat intermediate root was flattened:
  the intermediate never crossed a trust boundary, and HKDF's one-wayness
  already keeps a leaked leaf key from revealing siblings or the root.)

Golden test vectors pin the exact derived values for a fixed test
mnemonic; the derivation is version 1 and must stay stable
forever once installs exist — any scheme change must use new labels.

One recoverable root keeps operator backup instructions to a single
phrase for everything that can be deterministic, and purpose labels
prevent any two contexts from ever sharing a key. The root is acquired
once, by onboarding — generated, or adopted from a backup
([SPEC-admin-socket](./SPEC-admin-socket.md)); nothing else creates one,
so an install either has been onboarded or has no identity at all. The
phrase is never written to the log — generation is entirely silent —
and is retrievable only over the admin socket
([SPEC-admin-socket](./SPEC-admin-socket.md) `ShowMnemonic`).
Prompting the operator to back it up is an onboarding-UI job; nobody
reads logs for that.

## Signature scheme: secp256k1 BIP-340, two service keys

All protocol signatures made or verified by the FMan and FIs use
**secp256k1 BIP-340 Schnorr** with hex-encoded 32-byte x-only public
keys. This covers both directions of the commitment envelope
([SPEC-signed-envelopes](../../service-fleet-manager/specs/SPEC-signed-envelopes.md)):
FI-signed requests (`fi_id` is an x-only pubkey, bound to the seat at
creation) and FMan-signed commitment responses. The only Ed25519 keys
are the iroh endpoint identities above.

The cross-program contracts
([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md),
[SPEC-holder-trust-envelope](../../domain/specs/SPEC-holder-trust-envelope.md))
are uniformly secp256k1
Schnorr, and the FI-side spec already assumed secp256k1 BIP-340 FI keys;
an earlier Ed25519 envelope choice contradicted both and forced two
signature suites. HKDF output is not automatically a
valid secp scalar (rejection probability ≈ 2⁻¹²⁸); derivation may rely
on that being negligible, as the nostr-key derivation already does.

The two service keys are deliberately not collapsed into one: Nostr event
signatures carry no domain tag of their own, so a shared key would rely
on every consumer distinguishing contexts perfectly, and the ad key
already serves two document families (ads and peer attestations). The
cost of the second key is one derivation label. Because commitments must
remain verifiable from an ad alone, the advertisement explicitly binds the
service-sign pubkey to the ad identity
([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md)).

## Root-secret compromise and `api_auth`

`api_auth` derives from the root mnemonic like every other seat secret. The
mnemonic is the FMan's root secret and its compromise is already unrecoverable:
it yields the wallet and every seat identity regardless. Deriving the guardian
API password does not meaningfully widen that for a live host, because the
mnemonic is stored on the same host as the seats' fedimintd data directories —
and an attacker with that host already holds the guardian threshold shares
directly, without needing the admin API.

The accepted residual is narrower: a mnemonic retained separately from the
data root, as the backup procedure recommends, could leak without host access.
It would then permit remote extraction of guardian private config via
fedimintd's `get_guardian_config_backup`, which serializes `cfg.private` and
encrypts it under this same password. The mnemonic is protected as the root
secret and its loss is treated as terminal.

The mnemonic alone still does **not** recover a fleet. Every running seat's DKG
shares live in its fedimintd data directory and are not derived. Until a backup
command exists, the safe manual procedure remains: stop the daemon, copy the
complete data root, and retain the mnemonic separately
([ARCH-fleet-manager-product-boundary](./ARCH-fleet-manager-product-boundary.md)).
