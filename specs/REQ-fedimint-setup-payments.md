# REQ-fedimint-setup-payments: Federation setup payments need reliable rails

Source: Fedi product and business policy.

An FA must be able to pay each selected MG without flaky payment routing making
federation creation unreliable. Forming one federation can require roughly ten
separate FA-to-MG payments. Even a tolerable failure rate for one payment
compounds across the whole sequence and produces poor setup UX.

Direct Lightning payments do not provide enough confidence for this flow:
routing and liquidity failures are common enough to put the complete setup at
risk. Paid FA-to-MG setup therefore uses Fedimint ecash rather than direct
Lightning payments.

The common federation set used to make the ecash payer and payee interoperable
is chosen by
[SPEC-locked-payment](../crates/mg/specs/SPEC-locked-payment.md).
