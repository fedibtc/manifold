# Setup-payment publisher security notes

This is a one-shot, production-only operator CLI for the deployment key
custodian. It does not own the public key configuration, rotate or recover the
key, choose policy, or run continuously. Policy content, expected public key,
and file paths are trusted operator inputs; relays and all relay responses are
untrusted and may be stale, incomplete, or unavailable.

The publisher secret is the only sensitive input. It is accepted only from a
file or non-terminal standard input, never argv or the environment, and must
never be logged. Input memory is zeroized and parsed key material is dropped
immediately after signing, before receipt or network work. Receipts contain a
public signed event, but the newest receipt is operationally critical: it is the
publisher's durable replacement-order high-water mark and must be backed up by
the custodian independently of relays.

Exactly one publish or republish operation for this key may run across all
custodians and hosts. Relay preflight is not a distributed lock: concurrent
runs can share one previous receipt, sign conflicting events, and leave only
the NIP-01 winner authoritative. The custody procedure must serialize all
operations and retain that winning receipt. Revisit this invariant before
adding another custodian or automated publisher.

Before signing, the tool checks an independently obtained expected public key,
uses shared semantic admission, requires explicit acknowledgement for an empty
stop-set, and verifies complete authenticated current-address queries against
the asserted initial-publication state or previous receipt. It durably creates
a no-overwrite receipt before network I/O, publishes the same signed event to
every canonical Production-profile relay, and verifies both exact-ID readback
and current canonical address selection.

Current-address high-water verification authenticates signature, publisher,
kind, and exact `d` tag while treating content and timestamp as opaque. A newer
signed event therefore blocks overwrite even if this binary cannot parse its
future schema or consumers temporarily reject its future timestamp. New policy
content and the supplied prior receipt still receive full shared semantic
admission.

Every relay is attempted and any failure produces a nonzero exit. Recovery is
an explicit keyless republish of the same receipt; the tool never automatically
re-signs or retries. Connections and complete queries are bounded by timeouts.
Relay eventual consistency can still cause a visible partial publication, and
publisher-key compromise remains an out-of-band recovery incident. Revisit
these controls whenever relay/profile selection, the shared policy schema,
key-custody practice, or replacement semantics change.
