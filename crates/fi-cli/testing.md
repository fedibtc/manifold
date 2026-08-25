# fi-cli testing strategy

`fi-cli` is a development/test consumer of `fi-client`, so its tests protect
the consumer boundary without duplicating the library's state-machine and
policy coverage.

- Private unit tests cover argument parsing and preflight conversion into exact
  shared fi-client/domain types. They also cover small sequencing helpers where
  a failure must prevent a later library operation from being polled.
- Output unit tests pin exact JSON spelling, ordering, newline behavior, and
  stderr separation for stable staging scripts.
- Payer-adapter tests replay unordered and duplicate Fedimint transaction
  history across a simulated drop/reopen, bind the exact current operation and
  required output/change ranges, and require both mint-v2 bundle and mint-v1
  per-note rejection refunds to restore spendable outputs before replacement
  authority exists.
- Process-contract tests execute the compiled CLI to protect validation before
  state/resource access and to verify stdout/stderr behavior across the binary
  boundary.
- The paid `defe` E2E uses one large ecash note for seven sequential payments,
  reopens the real wallet after formation, reconstructs every exact current
  operation and validates/awaits its change through the adapter recovery
  primitive without a second submission, and independently
  audits receive/setup transaction fees, setup outputs, and returned spendable
  balance. Manual staging E2E uses only a disposable formed federation and
  test funds; external credentials and mutable staging availability keep it
  out of the ordinary automated suite.

The `--fi-spv2-account-file` path is an intentional fi-cli-only fixture source.
Tests should prove that preflight constructs the CLI's test
`FiFeeAccountProvider` and that the operation sends only the rate to
`fi-client`; production ownership and joined-client derivation belong to the
Fedi consumer.

Production-app UX, persistence hardening, exhaustive fi-client transitions,
and protocol-policy combinations are deliberately out of scope here. Those
belong to the production consumer or fi-client's own tests.
