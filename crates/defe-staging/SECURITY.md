# Defe staging security notes

`defe-staging` is local, single-user development and test infrastructure. It is
not a multi-tenant service and must use only dummy credentials and test funds.

- Trust the selected Defe socket and every selected resource/composer binary.
- The environment deliberately connects loopback Admin APIs and a local Nostr
  relay. Do not treat those endpoints or their fabricated development trust
  material as production identities.
- The staging root is mode 0700. `secrets.json` is mode 0600 and contains FMan
  passwords, the gateway password, and the FLIP bootstrap token. Ready output
  also prints each FMan password beside its operator-UI attach command; this is
  intentional for the disposable, single-user workflow. `env.json` contains no
  credentials, and ready output does not print gateway or FLIP credentials.
- Startup phases use bounded process and HTTP waits. A startup failure closes
  the composer connection, which releases every Defe lease; Defe retains the
  private temp root by default for diagnostics.
- Ctrl-C first marks a retained manifest stopped and non-ready, then exits and
  releases the connection-owned resources. The parent grants a bounded graceful
  shutdown interval before killing a stuck composer.
- `--keep-temp` retains logs, state, credentials, and the stopped manifest after
  teardown. Protect or delete that directory as test-sensitive material.
