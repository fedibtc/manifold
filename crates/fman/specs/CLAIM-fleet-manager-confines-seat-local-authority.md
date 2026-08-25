# CLAIM-fleet-manager-confines-seat-local-authority: Each seat remains confined to its local authority

For every official-daemon operation on a `Seat` whose immutable facts name seat
S, no operation causally triggered through that object obtains local
control-plane access to a distinct seat T registered in the same `Fleet`.

Local control-plane access includes reading or mutating T's immutable facts,
runtime mirror, durable row, or DKG history; using T's Fleet Manager-held API
credential, local API address, supervisor or process handle, derived process
identity, or data directory; and invoking an operation on T's `Seat`. The entry
domain includes production construction of S and every operation on an existing
S, including crash, restart, and concurrent FI or trusted operator activity.

Ordinary public Fedimint protocol communication is not local control-plane
access. It carries no sibling `Seat`, local API credential, supervisor or
process handle, or data-path capability.

## Status

Unverified.

## Assumptions

- SQLite and SQLx target and decode the stated rows atomically and durably; the
  official binary, data root, database, process memory, and host operator are
  trusted; and no alternate writer, unsafe code, or memory corruption bypasses
  safe Rust and module boundaries.
- The bundled `fedimintd` obeys its intended process and protocol contracts.
  Hostile protocol inputs, guardian codes, names, and metadata cannot make the
  unsandboxed child access sibling host paths, processes, localhost APIs, or
  environment beyond the resources supplied for S.
- CSPRNG output and mnemonic-rooted HKDF provide collision-resistant,
  independent key material, so distinct seat IDs do not accidentally share API
  credentials or derived identity and process keys.
- Rust and the operating system implement safe path joining, TCP binding,
  current-executable resolution, argument-zero replacement, and child-process
  handle ownership as documented. `current_exe` identifies the running official
  executable rather than a sibling resource, and a client, supervisor, or
  process given S's resources operates on S rather than an unrelated sibling.
