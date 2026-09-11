# Current argument

## Argument

Exact address, amount, outpoint ownership, and target-side claim checks establish
exclusive operation attribution, but they do not establish provider-wallet
provenance when the persisted operation has no txid. An ordinary third-party
output can be selected and later claimed by the gateway. The current
counterexample is recorded in
[`falsification-third-party-output.md`](falsification-third-party-output.md).

## Weakest links

A repair needs durable evidence that the selected output originated from the
provider-wallet submission, not only that it uniquely matches the operation.
