# SPEC-peerbadge-signing-protocol: Blind signing service interoperability

## Record justification

The native issuer, credential-app signer and holder, and independent RPC codecs
jointly implement this exchange, so no one handler or client owns its security
and byte-level interoperability contract.

## Exchange

ALPN `fedi/peerbadge-signing/1` carries the version-1
[fedi-iroh-rpc convention](../../fedi-iroh-rpc/README.md). Each call occupies one
bidirectional stream; FIN delimits each CBOR frame. The shared service types
own field order and enum spelling. Byte fields are CBOR byte strings, not
integer arrays. SDK authority, issuance-request, and issuance-response documents
travel as JSON strings rather than translated CBOR objects. Golden fixtures
under `tests/golden` are byte-level interoperability evidence.

A signer obtains a 32-byte opaque authenticated challenge valid for 60 seconds.
Clients echo its bytes unchanged. The BIP-340 signature signs SHA-256 of the UTF-8 domain
`peerbadge-signing/open-session/1`, a zero byte, the 32 nonce bytes, one level
byte, and the issuer's 32-byte x-only identity, in that order. There is no
additional signing hash. The issuer identity returned with the challenge lets
the signer bind its authorization to this issuer rather than another server.

Opening requires an allowed signer, valid fresh single-use challenge and
signature, a level in 1 through 9 within that signer's ceiling, and an available
hourly allowance. The response fixes the credential info and authority and
returns a random 32-byte session capability encoded as lowercase hex, valid
for 600 seconds. Anyone possessing this capability can authorize one issuance;
knowing an Iroh address does not authorize opening a session.

The holder constructs its blinded request against the offered authority and
info. Redemption signs only the session's fixed info. Until the original session
expiry, retrying the exact request JSON bytes returns the identical cached
response if their SHA-256 equals the first successful redemption's audited
digest. A different digest returns `SessionAlreadyRedeemed`. Clients retain the
original request and blinding factor across lost replies; reserializing JSON
may change its digest. Expiry discards the response and redemption returns
`SessionExpired` while the terminal record remains. Polling reports `Open`,
`Redeemed`, or `Expired`; removed/unknown capabilities return `SessionNotFound`.
Neither the offer nor polling discloses holder identity to the signer.

The server records opened, redeemed, replayed-redemption, expired, and
rejected-open events. A successful retry records `session_redeem_replayed`
with the original `request_sha256`. Invalid nonce length or MAC produces no
audit write; authenticated nonce rejections remain audited. Only a SHA-256
commitment to the issuance-request JSON may survive redemption in its audit;
request bodies and SDK parse/signing details must not enter diagnostics.
Audit/session capabilities require confidential storage even though they do
not disclose the unblinded holder.

This implements [REQ-peerbadge-signing-server](../../../specs/REQ-peerbadge-signing-server.md)
and is constrained by [GATE-production-compatibility](../../../specs/GATE-production-compatibility.md).
