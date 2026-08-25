# Falsification: `StartDKG` accepts bare guardian codes

At source baseline `5ad44677e12b7c29131efc8cb7236899db0c6e19`, clause 1 fails
while every immediate assumption is granted.

`GuardianCode` is explicitly a bare upstream Fedimint base32 `PeerSetupCode`.
`GetDkgCode` emits that type. `SeatLoop::validate_dkg_codes` decodes every peer
value as that type and recomputes only the local seat's code; `DkgCodeSet`
checks count, uniqueness, and own-code presence. It reads no peer FMan identity,
guardian-fee account, or endpoint signature and persists no account transcript.
The fleet tests' arbitrary distinct bare peer fixtures successfully enter DKG.

The current design deliberately moved account binding after DKG: formation peer
attestations carry each account, endpoint-key proofs bind them to final-config
peers, and `ProposeFormationMeta` verifies that evidence before the first fee
policy vote. That later mechanism limits the practical impact of this literal
counterexample, but it does not make the claim's `StartDKG` predicate true.
