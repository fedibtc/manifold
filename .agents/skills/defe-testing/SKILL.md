---
name: defe-testing
description: Start and use the local defe test-resource server, including the one-shot full test suite.
---

# `defe` local testing

`defe` allocates resources for tests that require local services.

## Persistent server

In a development shell, run:

```bash
just defe-serve
```

Keep it running while invoking `defe`-backed tests from another development
shell. If a test cannot connect to `.defe.sock`, start this server. Do not
rebuild or restart it during a test run.

## One-shot full suite

Run the full suite, including `FMAN_E2E` and `FLIP_E2E`, against the working
tree with:

```bash
just test-e2e-local
```

It starts its own `defe` server, reuses the incremental Cargo target directory,
and accepts additional `cargo nextest` arguments:

```bash
just test-e2e-local -E 'binary(=integration_daemon_smoke)'
```

Use `selfci check` or `.#ci.<system>.tests` for pure final verification.
