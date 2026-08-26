# defe security notes

`defe` is a local test resource server. It is intended for same-user development and CI jobs, not for multi-tenant or Internet-facing deployment.

- Keep the Unix socket and temporary directory private to the current user/job.
- Only pass trusted resource binary directories or explicit binaries using
  `--binary-path` or any `--*-bin` selector. This includes the
  `--defe-env-bin` composer, which executes with access to the private Defe
  socket and resource descriptors.
- Resource descriptors and exported `DEV_DEFE_*` variables are test infrastructure outputs and may reveal local temp paths and ports.
- All `defe` servers sharing an IPv4 loopback namespace must share one
  `DEV_DEFE_PORTALLOC_DATA_DIR` or be externally serialized. Independent
  ledgers are safe only with isolated network namespaces; otherwise servers can
  reserve identical startup ports and reach the wrong same-user test process.
  Nix CI enforces this within one test derivation graph through Linux network
  isolation or a Darwin dependency chain. Separate Darwin Nix invocations are
  not mutually serialized and must not overlap on one host.
