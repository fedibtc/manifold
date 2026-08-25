# CLAIM-allocation-existence-probe: Allocation existence probe

A caller that does not possess the requester key and details commitment of an
allocation cannot learn from the signed semantic content of a
`RequestLiquidity` response whether FLIP has an allocation for a chosen
federation.

The adversary knows a target provider key and federation id, controls an
arbitrary valid requester signing key and every request field, but has neither
the accepted requester's signing key nor its `details_payload_hash`, **and holds
no valid unrevoked FMan endorsement for the target federation**. The claim
concerns allocation *existence*, not item status, and is the privacy boundary
stated for status lookup in `SPEC-flip-rpc`. Timing and transport observations
are not claimed.

## Status

Unverified.

## Assumptions

- **A1 — response observation.** A caller can distinguish the signed public
  rejection code it receives.
- **A2 — target identifier knowledge.** A federation id can be known to a
  caller (for example from its invite code or public federation activity).
