# Falsification: an internally matching hostile policy is copied forward

At source baseline `5ad44677e12b7c29131efc8cb7236899db0c6e19`, the claim's
no-copy promise fails while every immediate assumption is granted.

A hostile federation threshold can install a canonical seat directory that maps
all final-config peer ids and guardian identities to attacker-chosen FMan keys and
unique guardian-fee accounts, with each attestation self-signed by its claimed
FMan key. It can install a matching payer-valid recipient list containing those
guardians at weight one, a distinct FI at weight four, the Guardian Verification
Fee at weight one, and an in-range rate.

Formation admission would reject that object because it verifies endpoint-key
proofs and requires the local peer to name this daemon's FMan identity. Those
proofs are intentionally not retained in consensus. Generic carry-forward later
runs only `FmanSeatBindings::verify_for_federation` plus recipient split
validation. These verify claimed-FMan signatures, final-config shape, and internal
account agreement, but do not repeat endpoint proof or local identity/account
binding. An unrelated generic metadata update therefore submits the hostile fee
object as this guardian's vote.

A hostile threshold already controls consensus, so this does not demonstrate new
funds-control power. It does demonstrate the exact in-claim outcome: this daemon
copies forward a policy which its local read identifies as not paying its
mnemonic-derived account.
