# CLAIM-hot-admin-config-rewrites-active-settlement-policy: Hot admin config rewrites active settlement policy

An Admin hot-config update cannot rewrite the funding-policy inputs used by an
already accepted allocation item or its later settlement work. Those later
effects must use persisted acceptance-time inputs, or the update must be rejected
until the affected work terminates. The adversary may race an authenticated Admin
update with workers.

## Status

Unverified.

## Assumptions

- **A1 — provider outflow matters.** A gateway withdrawal or target deposit under
  a different target/configuration is an economic effect of the active item.
