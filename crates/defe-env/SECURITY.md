# Defe environment security notes

`defe-env` is local, single-user development and test infrastructure. It is
not a multi-tenant service and must use only dummy credentials and test funds.

- Trust the selected Defe socket and every selected resource/composer binary.
- When `DIRENV_DIFF` is active, Defe also trusts the executable `direnv` selected
  from the caller's `PATH`. Before resource setup it resolves that binary and
  rejects `/.envrc` or `/.env`; at launch, `direnv exec /` restores the
  pre-development baseline before Defe applies its runtime overlay. The final
  launcher removes `DIRENV_DIFF`, `DIRENV_DIR`, `DIRENV_FILE`,
  `DIRENV_WATCHES`, and `DIRENV_IN_ENVRC`, then prepends generated tools.
- Before setup, Defe requires and trusts the exact `pnpm` selected from the
  caller's development `PATH`. Generated and advertised FMan UI commands pin
  that absolute path, so the restored neutral `PATH` cannot redirect them.
- The default shell starts in a canonical private work directory only after Defe
  verifies that no cwd ancestor contains `.envrc` or `.env`. Explicit commands
  retain the invocation cwd and relative-path behavior; their code remains free
  to activate an environment intentionally.
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
- `traffic` passes the private generated invite and Iroh routes only to the
  selected trusted Fedimint load tool. Its wrapper serializes one child at a
  time, caps users at 1,000 and duration at one hour, and permits at most 30
  seconds of timeout grace. It kills a timed-out child and treats timeouts or
  nonzero exits as failures. Mint and Lightning modes remain explicit
  unsupported failures. Traffic neither causes nor proves production Fedi
  payer-fee accrual.
- Startup phases use bounded process and HTTP waits. Before starting its async
  runtime or the first setup subprocess, the Linux composer starts a
  single-threaded broker bootstrap which creates an unprivileged user namespace
  and a nested PID namespace. The multithreaded composer remains outside both;
  trusted status-proxy helpers enter the broker user namespace and launch every
  setup or command subprocess in the PID namespace. A startup failure first destroys and reaps
  that complete namespace, then closes the composer connection and releases
  every Defe lease. Defe retains the private temp root by default for diagnostics.
- A setup-command timeout destroys and reaps the environment PID namespace
  before returning the timeout. Retry loops treat that timeout as terminal, so a
  stale setup command or its descendants cannot overlap a later attempt.
- The child command gets its own foreground process group, so Ctrl-C in an
  interactive foreground job does not tear down the environment. Child exit or
  external termination stops admission and terminates and reaps every
  environment descendant across foreground, background, and disowned
  job-control groups. Killing PID-namespace init makes the kernel kill every
  namespace member and reject new forks; a readable identity-stable init pidfd
  followed by reaping init proves completion without a `/proc` census. Only then
  does teardown mark a retained manifest stopped and non-ready and release
  connection-owned resources. This boundary prevents accidental leaks; it is
  not a sandbox against a malicious same-user command with authority to
  interfere with its composer. If pidfd inspection, signaling, or reaping fails,
  the composer retains its leases and retries rather than publishing stopped
  state or exiting.
- This boundary requires Linux pidfds and enabled unprivileged user namespaces.
  `defe env` fails before acquiring leases when either facility is unavailable.
  The single UID/GID mapping preserves the invoking user's file ownership but
  does not map supplementary groups into the environment user namespace.
- The broker passes argv as native Unix strings, applies the command's explicit
  environment overrides and working directory, and leaves stdio attached to its
  proxy. Namespace and launch-report descriptors are closed before executing
  environment code. The proxy reports the command's outer PID for terminal
  foreground transfer and exits with the same status or signal.
- Before the first allocation, a separate lifetime guard receives a duplicate
  of the composer's connected Defe socket. It never reads or writes the
  sequential protocol stream. It retains that descriptor across composer
  SIGKILL or abort and closes it only after the namespace-init pidfd proves
  kernel teardown complete, preventing abrupt composer death from releasing
  leases ahead of surviving commands.
- `--keep-temp` retains logs, state, credentials, and the stopped manifest after
  teardown. Protect or delete that directory as test-sensitive material.
