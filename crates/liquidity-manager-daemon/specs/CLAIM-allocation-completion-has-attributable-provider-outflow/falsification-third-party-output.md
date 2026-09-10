# Third-party output can stand in for provider outflow

At source change `moqkvurn`, a gateway funding send that returns an error before
broadcast is durably retained as `in_doubt` with its target address and amount
but no txid. If an ordinary third party pays that exact address and amount,
chain reconciliation can exclusively claim the output for the operation.
After the configured gateway claims the same output into the target federation,
the gateway worker can durably complete the item.

The dependency observations are truthful and SQLite is atomic and durable, so
the immediate assumptions are granted. The completed value is attributable to
the item's target output, but it was not caused to leave the provider wallet.
This is a source-derived counterexample; no runtime reproducer was added.
