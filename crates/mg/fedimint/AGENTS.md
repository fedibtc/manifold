# Manifold Guardian wallet agent notes

- Read `SECURITY.md` and the governing locked-payment and FA RPC records before
  changing payment, recovery, refund, or wallet persistence behavior.
- Preserve the payer/payee split: this crate is the MG's payee wallet only,
  exposed through `mg-core`'s `EcashWallet`. FA payer adaptation belongs to
  the consuming FA application, with `fa-cli` serving only as the reference
  implementation.
- This crate implements core's traits; it never defines MG policy. Anything
  deciding *what the MG owes or is owed* — guardian-fee accounts, the fee
  policy, prices — belongs in `mg-core`.
- Never log or serialize wallet roots, note secrets, private issuance requests,
  bearer ecash, payment evidence, or refund context.
- Preserve payee ordering: accepted seat/payment evidence is durable before
  claim; the durable seat-row replay check precedes the offer-epoch refusal
  check; every epoch change commits before a refusal depends on it; one quote
  cannot be both accepted and refunded.
- Add focused recovery, ambiguity, replay, and secret-formatting tests whenever
  one of these boundaries changes.
