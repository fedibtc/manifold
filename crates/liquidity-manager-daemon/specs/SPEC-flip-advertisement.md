# SPEC-flip-advertisement: FLIP provider advertisement and app-side trust gate

## Record justification

The advertisement contract binds the daemon's advertiser, Nostr relay
publication, and app-side verifiers outside this repository, so no single
implementation artifact can own it coherently.

`LiquidityProviderAdvertisement` is the provider's only public event and its
only ready signal. FLIP publishes it (provisional Nostr kind `37702`,
addressable, tags `["d","flip-provider-ad"]` and `["t","fedi-flip"]`) only
while setup and dependency validation pass, and withholds or withdraws it
otherwise; there is no public non-ready state. It exposes discovery, trust,
policy, and endpoint metadata only — never wallet capacity, gateway or
stability-pool inventory, allocation status, completion evidence, invite
codes, or credential bundles. Apps discover with
`{ kinds: [37702], "#d": ["flip-provider-ad"], "#t": ["fedi-flip"] }`; tags
are indexing hints only and are never trusted over the signed content.

Withdrawal restates the same document under the same coordinate with
`expires_at` equal to its `issued_at`, then requests deletion of that event.
Withdrawal takes effect by superseding the previous event; deletion only
attempts cleanup. A relay must keep just the newest event per addressable
coordinate, but may decline a deletion request. Clients can therefore discover a
withdrawn provider at most as an expired advertisement, which they reject under
the freshness rule below. They need no new message type. A client that never
learns of the withdrawal is in the same position as one reading any other
expired advertisement.

A withdrawal is durable. Publication requires validation to pass *and* no
standing withdrawal, so no automatic pass (periodic reconciliation, a config
change, or a relay refresh) returns a withdrawn provider to the relays. Only an
explicit operator republish does. Withdrawal changes no configuration, so
without this latch, the next reconciliation would undo the operator's decision
under a fresh signature while the operator believed they had left the market.
Withdrawal and ready explicit-republish requests commit their intent before
queueing for relay writes. The latest committed intent wins, including when a
request is canceled while queued; superseded queued operations need not send.
An in-flight publication cannot clear a newer withdrawal. Relay I/O that has
started may finish, and a canceled withdrawal may leave that event live until
expiry.

Publication and withdrawal share one serialized, time-paced writer. All callers
wait through a two-second startup guard, then each attempt waits for a
wall-clock second strictly newer than the last in-memory attempt. This assumes
one writer and a nondecreasing wall clock across restart, not durable ordering
through arbitrary clock rollback. Each clock wait has a separate, bounded budget that
starts after writer admission; slow relay I/O does not consume the next caller's
clock budget. A failed or canceled withdrawal retains its committed intent unless
an explicit republish supersedes it. Relay acknowledgment alone does not prove
visibility on arbitrary relays.

The signed document follows the shared MG/FLIP publication convention: a
typed `payload` (with `version`, `provider_pubkey`, `issued_at`,
`expires_at`, `supported_sources`, `holder_authorizations`, `policy`,
optional bounded `display` metadata, `api_endpoints` URLs, `api_versions`,
`relay_hints`) plus a `proof.signature` over the canonical JSON bytes under
`fedi-flip-advertisement/v1\0`
([SPEC-flip-canonical-payloads](../../service-liquidity-manager/specs/SPEC-flip-canonical-payloads.md)).
The Nostr event signing pubkey must equal `payload.provider_pubkey`. The
document is portable; Nostr is only one publication transport.

## Trust carriage

Provider identity is a service identity distinct from the operator's holder
key. It is **operator-imported**, not self-generated: the daemon derives its
endpoint identity from the provider signing key it was given
(`load_or_import_production_provider_identity`, the `install_provider_identity`
admin verb) and waits rather than minting one, so the advertised node id
survives a restart — see
[`ARCH-liquidity-manager`](./ARCH-liquidity-manager.md).

Trust rides inline: each `holder_authorizations` entry embeds a
holder-signed `HolderAuthorization` together with the backing Issuer
`SignedCredential`, per the MG carriage convention. Those entries are
enrolled from Holder-published kind-`37705` events
([SPEC-flip-holder-authorization](./SPEC-flip-holder-authorization.md)), which
carry the authorization and its badge together. The provider does not judge the
badge before publishing it, so an embedded envelope is a claim the app must
verify, never a trust conclusion. A provider with none enrolled is not ready
and does not advertise.

Issuer authorities come from app configuration or the shared attester-authored
kind-`37703` mirror; revocations are attester-authored kind-`37704`
`SignedRevocation` events on the relays listed in
`IssuerAuthority.issuer.revocation`. Direct provider-subject credentials (and
the reserved kind-`1382` bundle event) are a post-MVP extension; provider-key
rotation is post-MVP, and endpoint rotation is a fresh advertisement.

`policy` is machine-readable: `supported_networks` plus
`accepted_attester_policies`, each naming an `attester_pubkey` and a
`verification_requirement` (`all_trusted` or `consensus_majority_trusted`)
evaluated per [SPEC-flip-federation-trust](./SPEC-flip-federation-trust.md).

## App-side trust gate

Before sending private federation details, the app must verify:

- the advertisement signature under `provider_pubkey`, freshness
  (`issued_at` not far-future, `expires_at` not passed), and the wrapper
  pubkey match;
- at least one inline envelope passes: the holder authorization verifies with
  the SDK verifier and has `subject_pubkey = provider_pubkey`, its
  `credential_digest` equals the backing credential's digest, the credential
  verifies against an app-trusted issuer authority under the
  `fedi-trust-score-v1.0` schema with holder binding (`blind_msg`) equal to
  `holder_id_pubkey`, is unrevoked, and its trust level is acceptable under
  app policy;
- the chosen endpoint URL is covered by the provider signature, and the
  authenticated iroh identity reached equals the identity encoded by that URL
  (MVP endpoint URLs are `iroh://<node-id>?alpn=fedi%2Fflip%2Fpublic-liquidity%2F1`;
  extra query parameters are untrusted hints).

On failure the app must not send invite codes or other private federation
details.
