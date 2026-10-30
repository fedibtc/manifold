# SPEC-exact-payload-signatures: Exact-payload JSON signature envelope

## Status

Not implemented. No writer or reader uses this envelope yet. Its first user is
the MG advertisement v2 (kind `37709`) in
[SPEC-mg-nostr-events](../crates/nostr/specs/SPEC-mg-nostr-events.md), which is
also not implemented. The migration plan and the other candidate users are
tracked in fedibtc/manifold#247.

## Record justification

Signers (MG publisher, FLIP daemon), verifiers (fa-client, FLIP, programs
outside this repository) and every purpose that adopts the envelope must agree
on the same signed bytes, tags and checks, so no single implementation artifact
can own the contract.

## Envelope

A signed message is a JSON object with three members:

```text
{"signer": "<64 lowercase hex: BIP-340 x-only public key>",
 "payload": "<the serialized JSON payload, as a JSON string>",
 "signature": "<128 lowercase hex: BIP-340 Schnorr signature>"}
```

The signed BIP-340 message is the 32-byte digest

```text
SHA256( purpose_tag || payload_bytes )
```

where `payload_bytes` are the UTF-8 bytes of the decoded `payload` string
contents. The outer quotes and escape sequences are not signed. Verifiers do
not normalize Unicode or parse and reserialize the payload to derive the
signed bytes. A JSON string survives any reserialization by a conforming JSON
transport, such as a Nostr relay, because the decoded contents do not change
even when the escapes do. Because of this, the payload is carried as a string
and not as an embedded JSON object.

The signer must serialize the payload once and carry exactly that string. It
never re-serializes the payload after signing. A store or forwarder keeps the
envelope or the payload string verbatim. A typed projection is not enough.

`signer` sits outside the payload so a verifier can select the key before it
interprets any attacker-controlled payload. BIP-340 commits to the public key,
so a signature binds its signer. A payload therefore does not need to repeat
the signer. A purpose whose payload names its signer anyway must require the
two to be equal.

The envelope has no version or tag member. The verifier's context selects the
purpose tag: the Nostr kind and `d` tag, or the API, verb and direction. The
message never selects its own tag. A different envelope shape or algorithm
needs a new context, such as a new kind or API version, not a new member.

## Purpose tags

Tags follow the FLIP convention of fixed, NUL-terminated, versioned ASCII
strings (`types.rs` in `service-liquidity-manager`). Every tag of this scheme
has the form

```text
fedi-signed-json/<purpose>/v<N>\0
```

The `fedi-signed-json/` prefix keeps these tags disjoint from every legacy
domain (`fedi-fman-…`, `fedi-flip-…`). A signature made under a legacy
JCS/CBOR scheme can therefore never verify as an envelope signature, and the
reverse is also true. The NUL terminator makes tags prefix-free, because no tag
contains NUL. `<purpose>` names the owner and the message, and adds the verb
and direction where the message is part of an RPC. For example,
`flip-request-liquidity-request` and `flip-request-liquidity-response` are
distinct tags, so a response cannot be replayed as a request or as another
verb.

`v<N>` is the major version of that purpose's payload schema, and it is the
only home of that version. Payloads carry no `version` field. Additive changes
keep the tag. A change that old readers must enforce, or that changes an
existing field's meaning, needs a new `v<N>`. Over Nostr, it also needs a new
kind or `d` tag, so that old readers still find a message they understand.

Allocated tags:

- `fedi-signed-json/mg-advertisement/v1\0`: MG advertisement v2
  ([SPEC-mg-nostr-events](../crates/nostr/specs/SPEC-mg-nostr-events.md)).

## Verification order and limits

A verifier applies these steps in order and stops at the first failure:

1. Apply the transport's ingress bound before decoding. For role-fetched Nostr
   events, this bound is the 64 KiB normalized whole-event cap
   (`ROLE_FETCHED_EVENT_MAX_BYTES`), and it applies before the event signature
   is checked.
2. Decode the envelope. Reject a non-object, a missing or duplicate known
   member, or a malformed `signer` or `signature`. Reject a `payload` that is
   not a string or that contains an unpaired surrogate escape. Ignore unknown
   envelope members. They are unsigned, so they carry no meaning.
3. Reject a payload whose decoded contents exceed 65,536 bytes. The limit
   counts bytes, not characters. It applies to the decoded contents, not to the
   escaped form.
4. Verify the signature over the purpose tag that the context selects and the
   payload bytes.
5. Only then parse the payload as JSON with a 128-level nesting bound. Reject
   duplicate known names. Duplicate unknown names are skipped like any other
   unknown name. Numbers follow the rule below. Then validate the payload
   against its schema.
6. Apply purpose checks: bind the signer to the transport identity (for Nostr,
   the event author) and to the payload's identity field when it has one.
   Then apply freshness, authorization and semantic validation.

These bounds are independent. Escaping costs up to 2× per quote or backslash
inside the envelope. When the envelope is itself a Nostr `content` string, the
cost reaches 4× in the normalized event, and control characters expand further.
A payload that meets the 65,536-byte limit can therefore exceed a 64 KiB
whole-event cap. An admissible message fits every applicable bound. Writers
check the encoded size of the outer message, not only the payload size.
Verifying first narrows unauthenticated processing to the ingress bound and the
envelope decode. It does not replace any later check.

## Numbers

Signature verification never interprets numbers. Only the payload parse in
step 5 does, under one rule for every implementation.

- **Known number fields.** Every known number field in these schemas is an
  integer. A reader validates the decimal token exactly against the field's
  integer range. It rejects a token with a fraction or exponent, such as `1.0`
  or `1e3`, and a token outside the range. A token is never converted through a
  floating-point or other rounding representation. For example,
  `9007199254740993` in a `u64` field is that exact value, and a reader that
  cannot represent it exactly rejects the payload instead of rounding it.
- **Numbers inside unknown fields.** A reader checks only that the token
  matches the RFC 8259 number grammar, and then skips it. Magnitude and
  precision never decide acceptance, so `1e400` or a 500-digit integer in an
  unknown field is accepted and ignored. A malformed token such as `01`, `1.`
  or `NaN` makes the payload invalid JSON and is rejected. A decoder that
  buffers unknown values into a native number representation before skipping
  them does not meet this rule, because its own range would decide acceptance.

## Unknown fields

After verification, readers ignore payload fields they do not know, at every
nesting level. This rule gives both forward and backward compatibility.
Writers add fields only when the new field keeps every existing field's meaning
and its safe default when absent. A field that old readers would have to
enforce for trust, authorization, endpoint safety or payment terms is not an
additive change. It needs a new purpose tag version. Because the signature
covers exact bytes, unknown fields stay authenticated when a reader forwards or
stores the verbatim payload.

## Alternatives

- **Sign the bytes of an embedded JSON object, using a raw value.** Rejected.
  A JSON transport may legally reserialize objects, so the bytes do not survive
  transport. The decoded contents of a string do survive.
- **Opaque CBOR byte string.** Viable, but not chosen. It is compact, but it
  needs a text encoding for Nostr and loses readability (fedibtc/manifold#247).
- **Rely on the Nostr event signature alone.** Rejected. The documents are
  portable beyond Nostr (RPC, storage, forwarding), and an event signature has
  no purpose tag.
- **Carry the tag or the scheme version in the envelope.** Rejected. A
  message-chosen tag defeats domain separation, and a second version source
  must be kept in sync with the tag.
- **Reuse the legacy tags under the new signing input.** Rejected. Doing so
  silently reinterprets legacy signatures.
