# CLAIM-accepted-stability-allocation-requires-target-module: Accepted stability allocation requires target module

Before FLIP accepts a request containing a nonzero stability-pool allocation,
it verifies that the target federation's authenticated client configuration
contains a usable stability-pool module. Therefore a hostile FI with an
endorsed ordinary federation cannot make FLIP send provider wallet value into a
target client which has no `StabilityPoolClientModule`.

The FI controls a valid endorsed federation and request contents, scheduling,
and crashes. It cannot forge credentials, alter FLIP's database/provider wallet,
or act as Admin. Admin-configured FLIP source support is trusted.

## Status

Unverified.

## Assumptions

- **A1 — ordinary federation formation.** An FI can form an otherwise valid
  endorsed federation without an optional stability-pool module. FI formation
  supplies no custom module configuration; FMan rejects one, and its standard
  `fedimintd::default_modules` registry has mint, wallet, lightning, and meta
  only.
- **A2 — external effect.** A completed provider-wallet withdrawal to a target
  client's peg-in address is an irreversible provider outflow even if the
  target client subsequently cannot use its e-cash for the requested source.
