# GATE-selfci-local-development-cost: Routine SelfCI local-development cost

## Gate

Agents must not add work to routine `selfci check` without a new explicit user
request. Even when a user authorizes it, agents must measure and justify the
added local-development cost before adding it. Prefer a separate, narrowly
scoped GitHub Actions workflow for heavier validation, while treating its
performance as paramount.

## Justification

Developers run `selfci check` routinely during local development. Its latency
must remain proportionate to that use rather than accumulating incidental
validation.
