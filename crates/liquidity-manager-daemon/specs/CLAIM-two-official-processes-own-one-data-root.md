# CLAIM-two-official-processes-own-one-data-root: Two official processes own one data root

Two official normal or restore FLIP processes resolving to the same `data_dir`
cannot concurrently own money-moving, public, or restore surfaces for that root.
The adversary starts official processes concurrently but cannot rename, unlink,
replace, or otherwise mutate the protected root's lock path. The claim excludes
direct database tools and separately configured data roots.

## Status

Unverified.

## Assumptions

- **A1 — advisory lock exclusivity.** `try_lock` grants exclusive ownership for
  one open file description.
- **A2 — canonical root identity.** Official processes that are said to share a
  data root resolve `data_dir/flip.lock` to the same lock file.
- **A3 — protected lock path.** No actor replaces or unlinks `flip.lock` while an
  official process owns its root; otherwise an advisory lock protects its open
  inode rather than a newly created pathname.
