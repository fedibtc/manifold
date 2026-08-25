# fi-cli agent notes

- fi-cli is development/test-only tooling, not a supported production FI
  application or wallet. Keep its ordinary state handling proportionate to a
  trusted local operator and disposable test state.
- Read `SECURITY.md` before changing formation input ordering, resource opening,
  ordinary identity/state persistence, funding-token journaling,
  wallet-root-secret input, or their trust boundaries.
- fi-cli is a consumer of fi-client; it must not own a
  second formation state machine, signing scheme, trust policy, or FMan flow.
- The reference FI payer adapter lives here, not in the FMan wallet crate. Bind
  payer operation identity and recovery metadata to the exact quote, issuance
  set, mint generation, and module. Recover-only probes never create or fund a
  transaction, and ambiguous submission never authorizes a second spend.
- Keep refund preparation deterministic from the wallet root and exact quote
  binding. Private refund contexts stay non-serializable and non-printable;
  exact signed-refund settlement must be retry-safe.
- Identity and state persist in --state-dir for restart/E2E testing. Never
  print or log secret-key bytes; do not add production-grade persistence
  machinery without an explicit scope change.
- Keep the exact JSON stream contract in `specs/ARCH-fi-cli.md` stable;
  human-readable output may be improved independently.
- Follow `testing.md` when choosing unit, process-contract, or manual staging
  coverage for CLI changes.
- PR 83's one-shot CLI is protocol-call reference material only. Its ephemeral
  identity, no-persistence behavior, and direct orchestration are not this
  crate's architecture.
