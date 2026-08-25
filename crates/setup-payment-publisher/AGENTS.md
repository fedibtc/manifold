# Setup-payment publisher instructions

- Read [`README.md`](./README.md), [`SECURITY.md`](./SECURITY.md), and
  [`SPEC-setup-payment-federations`](../../specs/SPEC-setup-payment-federations.md)
  before changing signing, secret handling, receipt persistence, relay
  selection, publication/readback, or failure behavior.
- Production relays come only from the Production environment profile. Do not
  add a test key, relay override, fallback relay, or production command escape
  hatch.
- Keep tests isolated from Production; real relay coverage must use a leased
  local `defe` resource as documented in [`testing.md`](./testing.md).
