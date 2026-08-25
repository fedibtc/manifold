# Fleet Manager wallet agent notes

- Read `SECURITY.md` and the governing locked-payment and FI RPC records before
  changing payment, recovery, refund, or wallet persistence behavior.
- Preserve the payer/payee split: this crate is the FMan's payee wallet only,
  exposed through `fman-core`'s `EcashWallet`. FI payer adaptation belongs to
  the consuming FI application, with `fi-cli` serving only as the reference
  implementation.
- This crate implements core's traits; it never defines FMan policy. Anything
  deciding *what the FMan owes or is owed* — guardian-fee accounts, the fee
  policy, prices — belongs in `fman-core`.
- Never log or serialize wallet roots, note secrets, private issuance requests,
  bearer ecash, payment evidence, or refund context.
- Preserve payee ordering: accepted seat/payment evidence is durable before
  claim; the durable seat-row replay check precedes the offer-epoch refusal
  check; every epoch change commits before a refusal depends on it; one quote
  cannot be both accepted and refunded.
- Add focused recovery, ambiguity, replay, and secret-formatting tests whenever
  one of these boundaries changes.
