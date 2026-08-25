# CLAIM-private-data-escapes-public-exit: Private data escapes public exit

The following classified values cannot escape through the following enumerated
public exits: provider Nostr private key, bootstrap Admin bearer, secret-store
key, gateway Admin credential, bitcoind password, request invite code, and
request-carried FMan trust material/credential bundle, and target Fedimint peg-in
address; exits are the three public
RPC response constructors (`get_provider_info`, `request_liquidity`,
`get_allocation_status`), `LiquidityProviderAdvertisement`/`RelayPublishRequest`
content, the gateway/stability `CompletionEvidence` variants, normal/restore
unauthenticated `/health`, and every `tracing` macro invocation in the scoped
daemon files. The adversary sends arbitrary public requests and observes those
outputs and logs but cannot read local files or authenticated Admin responses.

## Status

Unverified.

## Assumptions

- **A1 — DTO serialization fidelity.** A response/event serializes only the
  fields of its selected DTO and its nested DTOs.
- **A2 — log sink boundary.** The observer sees values passed to tracing but does
  not read process memory or local secret files.
