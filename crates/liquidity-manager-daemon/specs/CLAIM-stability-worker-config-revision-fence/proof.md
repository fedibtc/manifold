# Current argument

## Argument

The worker checks the held client's exact config hash and resolves the stability
module before requesting a pooled peg-in address, then persists the returned
address for funding retries. This prevents simple handle substitution. It does
not yet establish that the checked hash remains the mint-time hash across awaits
or that a pooled address originated under that hash.

## Unresolved proof obligations

A3 permits an in-place broadcast-key config writer. The prior proof relied on a
nonexistent A4 to exclude its relevant starting state, and current production
uses pooled stateless addresses rather than the cited freshly minted address API.
No assumption may be invented to close either gap.

## Weakest links

Verification needs a granted invariant that freezes the exact checked config
through address origin, plus provenance for reused pooled addresses.
