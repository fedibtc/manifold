# SPEC-recipient-auth: Nostr NIP-98 management authentication

## Record justification

The auth contract binds the gateway's extractors, the mobile app's signing
client and key-derivation model, and operator security expectations, so no
single implementation artifact can own it coherently.

Management, registration, and legacy direct notification endpoints require
`Authorization: Nostr <base64(event)>`, where the event is a NIP-98-style
kind `27235` HTTP auth event. The gateway verifies signatures only and never
derives private keys.

Verification checks, all exact: the event signature; the signed method; the
signed `u` tag against the absolute URL built from
`PUSH_GATEWAY_PUBLIC_BASE_URL` plus the request path/query; a `payload` tag
over the raw request bytes, required for body methods; and a short freshness
window on the event timestamp. Accepted auth event ids are cached for the
full freshness window to reject exact replay in the single active process;
if the bounded replay cache is full of still-fresh ids, authentication fails
closed rather than evicting unexpired replay protection.

After signature/tag/timestamp and admission checks, but before replay-cache
insertion, every otherwise-valid event consumes a trusted-proxy-aware source-
prefix budget. A throttled source receives `429 auth_source_rate_limited` and
does not consume replay capacity; a distinct source remains serviceable. The
auth-source limiter is partitioned from all other limiter families and never
evicts live windows to admit a new source key.

The effective `recipient_id` is always the canonical lowercase hex Nostr
public key from the signed event. Request body/path/query recipient strings,
`npub`, NIP-21, uppercase hex, and legacy `app_id` equality are not identity
proof.

FCM-token ownership is durably bound to the stable installation id, not to one
app-root recipient forever. Fedi retains both values across an account switch,
so every valid signed registration for the exact pair is authoritative without
old-account cleanup. Registration mutations serialize, and the latest one to
commit owns the pair atomically in both the route and owner tables. Another
still-valid clone may submit the pair later and take ownership back; ownership
may oscillate until only one clone continues registering. A request using the
token with a different installation id remains a conflict until the current
route signs a rotation or unregister.

Public hook invocation is deliberately outside this scheme: it is authorized
by possession of the bearer-capability hook URL
([SPEC-hook-invocation](./SPEC-hook-invocation.md)).

Clients derive the recipient auth key from their app root-secret model with
label `fedi-push-gateway/recipient-auth-nostr/v1` and the same
environment-separation rule as
[`docs/fi-nostr-backups.md`](../../../docs/fi-nostr-backups.md). Key
rotation and multi-device recovery are handled by that client derivation
model; server-side in-place recipient-key rotation is not part of the server
MVP.
