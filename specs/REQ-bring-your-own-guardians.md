# REQ-bring-your-own-guardians: Federations formed from guardians the FA brings

Source: Fedi product decision, the approved first-iteration scope for
bring-your-own guardians.

## Status

Not implemented. Every seat still comes from verified selection
([ARCH-fa-client-discovery-selection](../crates/fa-client/specs/ARCH-fa-client-discovery-selection.md)),
onboarding still requires a Holder authorization, and the records linked below
carry their own `## Status` for the parts that do not apply yet.

## Requirement

An FA must be able to form and maintain a Manifold federation with guardians
the FA knows personally, without Fedi verification of those guardians. This
is a second formation path beside the Fedi-verified path, which stays
unchanged.

- **Code-based path.** The FA supplies every seat through a guardian code
  that the MG operator generates and shares privately (by QR code or text).
  The protocol object is a SeatGrant
  ([SPEC-seat-grant](../crates/mg/specs/SPEC-seat-grant.md)). There is no
  discovery, no automatic selection, and no mixing of code-supplied and
  selected seats in one federation.
- **Delegation.** Sharing a code delegates to the receiving FA every decision
  an FA makes about that seat: the other guardians, the federation size, later
  replacements (including moving that seat to another MG), and gateway
  attachment. Within that delegation the operator needs no further protection
  from the FA. Leaked codes behave the same way and need no protection beyond
  revocation by the operator.
- **Free.** Code-based setup and replacement carry no seat price and need no
  setup-payment federation.
- **Size.** A code-based federation has 4 through 20 guardians. The Fedi-verified
  path keeps its range. Operators get no new control over permitted sizes.
- **Operators without a badge.** An MG can be set up and operated with no badge
  and no Holder authorization. Such an MG is not publicly discoverable. One MG
  may serve code-based seats, Fedi-verified seats, or both, and advertises at
  most one public badge. A badged MG that serves a code-based federation
  reveals its badge to that federation's members. That is accepted.
- **Fees and telemetry.** Fedi's guardian-fee share and the existing telemetry
  apply to code-based federations as well. The fee is explained as supporting
  privacy-preserving diagnostics, access to the Fedi-verified guardian pool
  when used, and protocol improvements, bug fixes and security maintenance.
  Telemetry from MGs without a badge must not be able to degrade telemetry
  from badged MGs
  ([ARCH-cloud-mg-telemetry](./ARCH-cloud-mg-telemetry.md)).
- **Lightning.** The FA may attach a gateway that a provider runs and funds
  separately. A federation uses either such a custom gateway or Fedi-verified
  FLIP liquidity, not both
  ([SPEC-fa-post-formation-liquidity](../crates/fa-client/specs/SPEC-fa-post-formation-liquidity.md)).
  FLIP serves any federation whose operators are distinct trusted badge
  holders, however it was formed
  ([SPEC-flip-federation-trust](../crates/liquidity-manager-daemon/specs/SPEC-flip-federation-trust.md)).

## Outside this requirement

Custom signers or trust thresholds; multiple public badges per MG; a second
MG identity for code-based seats; mixed code-based and selected groups;
federation-specific telemetry controls; new replacement mechanics; manual
federation funding in FLIP.
