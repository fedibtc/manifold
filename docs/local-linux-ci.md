# Running the Linux CI locally (Docker)

CI runs on Linux. On a macOS workstation you cannot build the `.#ci.*`
derivations directly:

- there is no native `x86_64-linux` / `aarch64-linux` builder configured, and
- unless you are a Nix `trusted-user`, the `fedimint.cachix.org` binary cache is
  ignored, so the fedimint closure would compile from source (very slow), and
- `selfci` and some `iroh` dependencies do not build on darwin at all.

The fix is to run the build inside a `nixos/nix` **Docker container**. Inside the
container we are root, which *is* a trusted Nix user, so the cache is honored and
the whole fedimint closure is fetched **prebuilt**. The container's Linux userland
also sidesteps the darwin-only build failures. Same Mac, same CPU — just a Linux
userland in a box, which is what CI actually runs on.

## Quick start

```bash
# One lint/compile check of the whole workspace (fits the default Docker RAM):
just ci-docker clippy

# Several checks at once:
just ci-docker cargoFmt clippy leanProofs

# The full test suite (see "Memory" below — needs ~24 GiB):
just ci-docker tests
```

Equivalently, `./scripts/ci-in-docker.sh <target> [...]`.

A target is any attribute under `.#ci.<system>`. List them with:

```bash
nix eval .#ci --apply builtins.attrNames
```

Common targets: `cargoFmt`, `clippy`, `leanProofs`, `tests`,
`cargoDependencyHygiene`.

When a build succeeds the script prints the realized store path
(e.g. `/nix/store/…-decentralized-federations-ci-clippy-0.1.0`) and exits 0.

## Prerequisites

- **Docker Desktop running.**
All repository source inputs are public, so this workflow needs no GitHub
repository access token.

## Memory (important for `tests`)

`tests` builds the `fedimintd` release binary with `lto=fat` — a *single* `rustc`
process whose peak memory exceeds 8 GiB. Docker Desktop's default (~8 GiB on this
machine) is not enough, and no amount of `--max-jobs` limiting helps because the
ceiling is one process.

Raise it in **Docker Desktop → Settings → Resources → Memory → ~24 GiB → Apply &
Restart**. Your host RAM is the only real limit. The named volumes survive the
restart, so the prebuilt closure is not re-downloaded.

The lighter targets (`clippy`, `leanProofs`, `cargoFmt`) fit comfortably in the
default allocation.

## How it works

- **`aarch64-linux` vs `x86_64-linux`.** The script picks the system that matches
  the container's native arch (from `uname -m`) so there is no Rosetta emulation:
  Apple Silicon → `aarch64-linux`, Intel → `x86_64-linux`. Override with
  `CI_SYSTEM=…`. Building an `x86_64-linux` derivation in an arm64 container fails
  with `platform mismatch`.
- **Persistent caches.** Two named Docker volumes are created once and reused:
  `defe-nix` (`/nix`, the store + prebuilt closure) and `defe-home` (`/root`, the
  flake and cargo caches). First run downloads the closure (~tens of GiB); later
  runs are fast.
- **Substituters.** The container's `NIX_CONFIG` enables `cache.nixos.org` and
  `fedimint.cachix.org` with their public keys. The flake's own `nixConfig`
  substituter line is ignored as "untrusted" — that is expected and harmless
  because the global config already enables the cache.

## Environment overrides

| Var | Default | Meaning |
| --- | --- | --- |
| `MAXJOBS` | `2` | nix `--max-jobs` |
| `CORES` | `6` | nix `--cores` |
| `IMAGE` | `nixos/nix:latest` | container image |
| `CI_SYSTEM` | autodetected | force `aarch64-linux` / `x86_64-linux` |

## Troubleshooting

- **Exit 137 / `signal: 9, SIGKILL` while compiling `fedimintd`** — OOM. Raise
  Docker memory (see above).
- **`platform mismatch: Required x86_64-linux, Current aarch64-linux`** — you
  asked for the wrong system; let the script autodetect or set `CI_SYSTEM`.
- **`ignoring untrusted substituter 'https://fedimint.cachix.org'`** — only a
  problem on the *host*; inside the container we run as a trusted user, so this
  warning does not appear there.
- **First run is very slow** — it is downloading the prebuilt closure into the
  `defe-nix` volume. Subsequent runs reuse it.
