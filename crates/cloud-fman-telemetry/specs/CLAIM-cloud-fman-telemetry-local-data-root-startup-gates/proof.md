# Proof: Cloud FMan telemetry local data-root startup gates

## Scope and model

Scope:
`crates/cloud-fman-telemetry/src/{cipher,config,config_tests,data_root_lock,server,store}.rs`,
`crates/cloud-fman-telemetry/migrations/*.sql`,
`crates/cloud-fman-telemetry/Cargo.toml`, `Cargo.lock`, `flake.nix`, and this
claim and proof.

The crate does not enable the test-only `defe-test-support` feature by default,
and `flake.nix` selects the ordinary package binary for the production
collector archive without adding that feature. The model covers that production
build starting with arbitrary parsed command-line/environment configuration,
key-file contents and modes, and preexisting paths under its configured local
root. It permits a competing cooperative process on that same root. It excludes
the additional test-support configuration path, hostile same-UID pathname
mutation, processes using another root, remote filesystems, split/overlay
mounts, and orchestrator behavior.

## Axioms

The three assumptions in
[the claim](../CLAIM-cloud-fman-telemetry-local-data-root-startup-gates.md)
supply filesystem metadata, cross-process locking, pathname stability, SQLite,
cipher, parser, and operating-system semantics.

## Argument

1. **[code, test] Production configuration gate.** `serve` calls
   `Args::validate` before acquiring the data root. Without the test-support
   feature, the validator rejects a public base URL other than a
   credential-free HTTPS origin with no query, fragment, path, or trailing
   slash; equal listeners; non-loopback private bind without the isolation
   assertion; empty or over-128-byte key id; lease at most 60; metrics cadence
   other than 900 or 1800; metrics concurrency outside `1..=32`; empty or
   over-128-byte source version/hash; production `REPLACE_ME` source values;
   journal cadence outside `10..=86400`; journal concurrency outside `1..=32`;
   quota outside 1 MiB..=10 GiB; retention outside `1..=30`; and an
   unrecognized environment. Clap separately parses socket/IP syntax and
   trusted proxies. `canonical_method_labels` remains an operator assertion.
   The `defe-test-support` branch adds direct-endpoint test configuration and is
   explicitly outside this production-build claim.
2. **[code, test] Root and known-path gate.** `DataRootLock::acquire` creates a
   missing root as mode `0700`, otherwise requires a real effective-UID-owned
   mode-`0700` directory, and rejects symlinks and nonregular known lock/SQLite
   paths. After successful `Store::open`, `secure_sqlite_files` has made each
   present known regular lock/SQLite file effective-UID-owned mode `0600`.
3. **[code, test, assumption] Cooperative same-root exclusion.** `acquire`
   takes a nonblocking exclusive `fs2` lock. The synchronized child-process
   regression forces a second operating-system process to observe exclusion
   and verifies reacquisition after release. Under the filesystem premise,
   this covers cooperative processes using the same configured root.
4. **[code, test] Lock lifetime.** `serve` retains the lock in supervisor and
   HTTP state. The supervisor lifetime test observes that another acquisition
   remains excluded until those owners release it.
5. **[code, test, assumption] Key and persisted identity.** `read_key` rejects
   group/other-accessible files and byte strings other than length 32.
   `Store::open` connects and migrates, then rejects a stored environment/profile
   revision, secret format, key id, or authenticated sentinel mismatch under
   the SQLite and cipher premises.

## Evidence boundary

Selecting a different valid root makes the same gates apply to that root. The
gate does not prove immutable mounting, exact key mode or ownership,
single-volume layout, encryption at rest, network isolation, a single replica,
or that identity rejection precedes every database write.

## Residuals

A malicious same-UID process, remote or incorrectly locking filesystem,
different configured root, overlay below a valid root, and orchestrator
multi-active deployment are outside the quantified model. Migrations before
persisted-identity rejection are outside the successful-startup predicate's
guarantee.

## Weakest links

Filesystem and process behavior, SQLite, and authenticated encryption remain
axioms. Validator completeness and startup ordering are `code`; focused tests
pin the high-risk non-loopback, production-placeholder, pathname, lock, and
identity paths.
