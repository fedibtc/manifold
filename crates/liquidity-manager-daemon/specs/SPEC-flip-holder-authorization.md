# SPEC-flip-holder-authorization: Holder authorization of a FLIP provider

## Record justification

The enrollment contract binds the Holder miniapp and credential SDK outside
this repository, the operator console that shows the authorization request, and
the FLIP daemon that admits the published event, so no single implementation
artifact can own it coherently.

## Authorization request

A provider identity is a service identity distinct from the operator's Holder
key ([SPEC-holder-trust-envelope](../../domain/specs/SPEC-holder-trust-envelope.md)).
Trust attaches when a Holder signs an authorization naming that identity.

The operator console shows the SDK `HolderAuthorizationRequest` as a QR or
deep link, carrying the provider pubkey as `subject_pubkey` and nothing else.
It is not a Nostr event. The Holder app selects the badge locally, signs the
authorization, and publishes it. The console reads the provider pubkey and the
resulting enrollment state from the Admin API
([SPEC-flip-admin-api](./SPEC-flip-admin-api.md)); there is nothing to show
before an operator installs the provider identity.

## Published event (37705)

The Holder publishes one addressable event per authorization, authored by the
Holder pubkey:

```text
37705 Holder-published FLIP provider authorization
      (d = "flip-authorization:<provider_pubkey>:<credential_digest>",
       t = "fedi-flip-authorization", p = <provider_pubkey>)
```

The `content` carries `{version, holder_id_pubkey, holder_authorization,
signed_credential}` — the holder-signed authorization plus the backing trust
badge inline, so admission needs no second fetch. The subject pubkey is part of
the `d` coordinate, so one Holder authorizing several providers with one badge
publishes one event per provider rather than replacing its own previous
authorization.

This is the same event kind FMan uses
([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md)): the
document is one Holder authorization over a service subject, and only the
addressing differs. The FLIP `d` prefix, hashtag, and `p` tag keep the two
indexes disjoint, so a filter for one service can never match the other's
publication. `credential_digest` is the credential-SDK `CredentialDigest` wire
form, the same encoding the FMan variant and the attester revocation `d` tag
use.

Tags are indexing hints only. Admission parses and verifies event content, and
never trusts a tag.

## Admission

The daemon admits a candidate only when the event signature verifies, the
content's holder pubkey equals the event author, the SDK authorization proof
verifies, the signed statement's holder equals that same author, its
`subject_pubkey` equals this provider's pubkey, the inline credential's revealed
holder binding equals that same holder, and its `credential_digest` equals the
digest of that credential.

Admission establishes exactly that: a holder signed a statement naming this
provider, covering a credential payload issued to that same holder. It
deliberately does **not** judge the badge — the credential's issuer proof,
issuer trust, schema, and revocation state are not consumed. Complete envelope
verification belongs to the relying app, which
[SPEC-flip-advertisement](./SPEC-flip-advertisement.md) requires before it
sends private federation details.

The holder-binding comparison is the one part of the badge admission does read,
and it earns its place: without it a hostile key can sign a valid statement
naming a victim's credential digest, attach the victim's published credential,
and displace the victim's retained authorization with a later-dated copy. Every
other check passes, because the statement is genuinely signed and the digest
genuinely covers the attached credential. The comparison needs no issuer trust,
so it does not draw the daemon into judging the badge.

Two consequences follow and are accepted as the cost of one enrollment protocol
shared with FMan. A badge revoked after enrollment stays in the advertisement
until its authorization is replaced. Any holder of a genuine badge can place an
envelope there without being invited. Neither is trust: a consumer that skips
its own verification is already unsafe against an FMan advertisement.

Event kind, timestamps, and tags are not admission checks. The fetch filter is
an untrusted relay hint, so an off-kind or wrongly tagged event still has to
carry a valid inner authorization to be admitted.

## Retention and reconciliation

The complete signed event is retained, not the extracted envelope, and every
read re-runs admission against the current provider identity. A stored row is
therefore never a trusted assertion on its own.

The credential digest is the retained identity: one Holder authorizing this
provider for one badge occupies one row, and a republished authorization
replaces it only when its signed `issued_at` is strictly greater. That makes a
replayed older authorization inert once a newer one is enrolled. It is not a
freshness check — nothing compares `issued_at` against the clock, so the first
authorization admitted for a credential is whatever the relays served, however
old.

One reconciliation runs when a runtime generation starts, once a provider
identity exists. Without it a provider only ever reads when an operator opens
the authorization screen, so a deployment whose operator never opens it never
reads at all, and a restarted one waits indefinitely while the authorization a
Holder published was readable the whole time. Readiness waits on enrollment, so
that is the difference between advertising and not. Later reconciliations are
operator-driven: an authorization is published once and retained durably, so
there is nothing to poll for.

Reconciliation unions candidates
across the relays the deployment environment pins, under a bounded per-relay
candidate cap. Those are the relays a Holder app publishes to, and are not the
operator's advertisement relay configuration: reconciling against the latter
would report an answering relay and no candidates while the Holder's event sat
somewhere the daemon never queried. Relay
answers are additive: any relay may supply a candidate, and each still passes
admission locally, so answering promotes nothing. A relay that fails is
reported and does not fail the reconciliation, because one unreachable relay
must not block enrollment when another served the authorization. A relay
controls omission, so it can delay enrollment indefinitely, but omission cannot
empty what is already enrolled.

The enrollment state an operator reads distinguishes four situations, because
"no authorization" otherwise conflates three of them: nothing has been read yet,
a read completed and found nothing, and every relay failed. An enrolled
authorization outranks all three — the rows are durable and re-verified before
every use, and the advertisement still carries the envelope, so an empty or
failed read never reports a provider as unauthorized.

Admitted envelopes are carried inline in the provider advertisement. A
deployment with none enrolled is not publicly ready, so reaching ready for the
first time requires a successful relay read.
