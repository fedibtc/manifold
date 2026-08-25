# Falsification: the configured LNURL does not bind the Lightning payee

At source baseline `5ad44677e12b7c29131efc8cb7236899db0c6e19`, the literal
no-adversarial-destination payout clause fails while every immediate assumption
is granted.

The payout job correctly persists the operator-configured Lightning address or
LNURL string before wallet work and makes that string immutable. `lnurl_pay`,
however, follows the remote service's callback and accepts any BOLT11 invoice of
the requested amount. Native v1/v2 metadata, replay, and observation bind the
original string but establish no relationship between that string and the
invoice payee or payment hash.

A trusted operator can configure Alice's address while its compromised LNURL
service returns Mallory's correct-amount invoice. FMan then pays Mallory and
records Alice's string. The immediate assumptions trust operator control of the
configured string and the pinned client after it receives an invoice; they do
not trust the remote LNURL service.

This is a missing trust/product boundary, not a refund/reclaim bug. Repair requires
either narrowing the promise to the queried LNURL service label, adding an
explicit trusted-service premise, or implementing a verifiable final-payee
binding. This verification does not choose among them.
