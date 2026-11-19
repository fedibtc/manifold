# Setup-payment publisher security notes

This is a one-shot, production-only operator CLI for the deployment key
custodian. It does not own the public key configuration, rotate or recover the
key, choose policy, or run continuously. Policy content, expected public key,
and file paths are trusted operator inputs; relays and all relay responses are
untrusted and may be stale, incomplete, or unavailable.

The publisher secret is the only sensitive input. It is accepted only from a
file or non-terminal standard input, never argv or the environment, and must
never be logged. Input memory is zeroized and parsed key material is dropped
immediately after signing, before receipt or network work. A receipt is the
public signed event, and the previous receipt is the publisher's replacement-order
high-water mark. Custodians do not need a private copy of it: they take the
current event from the Fedi-operated canonical relay. That copy is sound because
a run counts as done only after every canonical relay returned the event, and
preflight rejects a previous receipt that any relay has superseded. The custodian
still checks that the fetched content is the policy they expect, because the new
policy is usually an edit of it: if every relay lost the newest event, an older
one would pass preflight, and editing it would silently revert the lost change.
The receipt a run saves must be kept until every relay has verified it, because
keyless republish needs it.

Exactly one publish or republish operation for this key may run across all
custodians and hosts. Relay preflight is not a distributed lock: concurrent
runs can share one previous receipt, sign conflicting events, and leave only
the NIP-01 winner authoritative. Custodians serialize by agreeing with each
other before each run; the winning event is then the one on the relays. Revisit
this invariant before adding another custodian or automated publisher.

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
