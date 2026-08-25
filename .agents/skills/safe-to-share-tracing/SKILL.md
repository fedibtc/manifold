---
name: safe-to-share-tracing
description: Audit a tracing event marked `safe_to_share = true`. Use whenever adding, removing, reviewing, or changing that annotation or any value the annotated event formats.
---

# Safe-to-share tracing

`safe_to_share = true` declares that the complete tracing event is safe to share:
it contains neither security secrets nor privacy-sensitive data.
Events without the literal annotation make no such declaration. Preserve the
event's ordinary message and fields when they pass the audit.

## Audit the complete event

The declaration covers:

- the rendered message;
- every event-local field;
- every value reached through `Display`, `Debug`, an error source chain, or
  `format_args!`; and
- the operational fact conveyed by the event occurring.

For secrecy, audit emitted bytes: a fixed event whose firing varies with an
operation involving a secret does not serialize that secret. Also check what the
event's occurrence reveals, but apply the privacy boundary below: ordinary
protocol and operational correlation is allowed.

For each added or changed annotation:

1. Enumerate the message and every event-local field.
2. Trace each value to its constructors and inputs. Do not stop at a wrapper's
   `Display` or `Debug` implementation.
3. Check the security and privacy classes below, including values that a
   dependency, remote peer, or supervised child may embed.
4. Check whether attacker-controlled values are bounded and whether their
   contents can carry secrets, rejected private data, or identifiers outside the
   allowed classes.
5. Annotate the existing event unchanged when the whole event passes. Otherwise
   narrow the unsafe field or leave the event unannotated.
6. Revisit existing annotations when changing a formatted type, error chain,
   child logging contract, or upstream value source they depend on.

The annotation is an assertion, not a sanitizer or authorization to collect.
Previous local logging and public protocol visibility do not by themselves
prove that a value is safe to share.

## Security-sensitive values

At minimum, reject data derived from:

- the FMan root mnemonic, seeds, or derived private keys;
- seat API authentication or guardian private configuration and key shares;
- bearer ecash, OOB tokens, notes, spend/blinding keys, private issuance
  material, or prepared refunds;
- bitcoind credentials, operator passwords, or session cookies;
- private invites or API bearer secrets;
- admin request or response bodies that may carry any of the above;
- process environment, argv, database contents, wallet databases, seat data
  directories, core dumps, or backups;
- hashes, fingerprints, encodings, truncations, or error text derived from a
  secret. Hashing is not redaction.

Treat dependency errors, remote rejection text, FI-controlled strings, and
fedimintd output as potentially sensitive until their complete provenance is
audited. Verbatim child output does not become non-sensitive merely because the
parent wraps it in another tracing event.

## Privacy boundary

This policy minimizes personal and operator-private data; it does not promise
anonymity or unlinkability. It deliberately allows correlation through opaque
operational and protocol identifiers.

Allowed after provenance review:

- opaque seat, federation, quote, event, transaction, and operation identifiers;
- public keys and identifiers intentionally exposed by the protocol;
- lifecycle events, exit codes, bounded error categories, counts, and durations;
- publicly advertised endpoints and public environment-profile routing; and
- fixed diagnostic messages.

At minimum, reject:

- names, contact details, physical locations, and other personal data;
- operator names, identities, and operator-supplied free-form text;
- identifiers that embed or derive from personal data or secrets;
- free-form FI, peer, child, or dependency text whose complete provenance has
  not been established;
- private request or response bodies and other private payloads;
- filesystem paths, usernames, process identifiers, environment values, private
  endpoints, and host-specific configuration; and
- error text or dependency output that may embed any rejected value.

An allowed identifier does not become prohibited merely because it links events,
but do not describe an annotated event as anonymous or unlinkable. The
annotation can only classify one event. A future collector must separately
specify operator intent, recipient, minimization, retention, transport,
timestamps, and cross-event aggregation.

## Examples

This event can be marked after confirming that `seat_id` is an opaque protocol
identifier and the exit status cannot encode rejected data.

```rust
tracing::warn!(
    safe_to_share = true,
    seat_id = %seat_id,
    exit_code = ?exit.status_code,
    "seat fedimintd exited"
);
```

This event cannot be marked: its complete contents are authored by the child.

```rust
tracing::info!(safe_to_share = true, "{child_line}");
```

An error chain is neither automatically safe nor automatically unsafe. Walk
every constructor and dependency boundary it can contain. If its provenance is
not established, leave the event unannotated.

## Finish the audit

Before finishing:

- search the touched scope for every `safe_to_share = true` event affected by the
  change;
- check the result against
  `crates/fman/specs/CLAIM-fleet-manager-confines-secret-dependent-content.md`;
- if the audit finds another current content exit, add focused falsification
  evidence beside that claim rather than weakening the security boundary; and
- report any event left unannotated because its provenance could not be
  established.
