# CLAIM-flip-stability-provider-independent: Stability provider is represented independently from FLIP

In the supported production configuration, FLIP acts only as a facilitator for
stability-pool provision: each stability allocation identifies and authenticates
a separately represented stability provider, draws principal from that
provider's separately configured funding authority, and deposits into a provider
account controlled independently from FLIP's target-client keys. FLIP cannot
substitute its own configured wallet or locally derived provider-account
authority.

This claim concerns technical representation and enforcement. It does not infer
legal or beneficial ownership from possession of a wallet credential, daemon
data directory, or target-client secret.

## Status

Falsified: ordinary stability allocation draws from the one configured gatewayd
and deposits through the FLIP-managed target client's locally derived
`AccountType::Provider` authority; no independent stability-provider funding or
account authority is represented
([evidence](CLAIM-flip-stability-provider-independent/falsification-current-deployment-control.md)).

## Assumptions

- The official production daemon, configuration, Public API, and Admin API are
  used without out-of-band database or process-memory changes.
- Technical possession or control of a credential is not by itself proof of the
  controller's legal or beneficial ownership.
