# Defe environment security notes

`defe-env` is local, single-user development and test infrastructure. It is
not a multi-tenant service and must use only dummy credentials and test funds.

- Trust the selected Defe socket and every selected resource/composer binary.
- The environment deliberately connects loopback Admin APIs and a local Nostr
  relay. Do not treat those endpoints or their fabricated development trust
  material as production identities.
- The environment root is mode 0700. `secrets.json` is mode 0600 and contains FMan
  passwords, the gateway password, and the FLIP bootstrap token. Ready output
  also prints each FMan password beside its operator-UI attach command; this is
  intentional for the disposable, single-user workflow. Generated mode-0700
  wrappers also contain the exact dummy credentials needed to select their
  service. `env.json` contains no credentials, and ready output does not print
  gateway or FLIP credentials.
- `fees synthetic-remit` stores its dedicated payment-wallet root secret and
  sealed remittance metadata under that private root. It passes only their file
  paths to `fi-cli`, never their contents, and prints no operation or wallet
  secrets. Its wrapper serializes the complete preparation action and every
  generated `fi-cli` invocation, so one owner accesses a wallet database at a
  time.
- Synthetic preparation directly creates a remittance in the disposable
  federation. It is intentionally not evidence of production payer accrual,
  weighted split calculation, accumulation, or scheduling.
- Startup phases use bounded process and HTTP waits. A startup failure closes
  the composer connection, which releases every Defe lease; Defe retains the
  private temp root by default for diagnostics.
- The child command gets its own foreground process group, so Ctrl-C in an
  interactive foreground job does not tear down the environment. Child exit or
  external termination first marks a retained manifest stopped and non-ready,
  then reaps the child group and releases connection-owned resources.
- `--keep-temp` retains logs, state, credentials, and the stopped manifest after
  teardown. Protect or delete that directory as test-sensitive material.
