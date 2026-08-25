# Proof: A compromised guardian child is contained



Scope: `crates/fman/core/src/{admin,db,fleet,identity,seat_process}.rs`,
`crates/fman/core/migrations/**`,
`crates/fman/fedimint/src/lib.rs`,
`crates/fman/bin/src/main.rs`,
`crates/fman/specs/{ARCH-fleet-manager,ARCH-fleet-manager-seat-processes}.md`

## Claim

After an officially spawned seat `fedimintd` becomes arbitrarily malicious,
it cannot read or modify fleet-level assets: the root mnemonic or SQLite
database, wallet state, the admin socket, or another seat's data and authority.

The child may issue arbitrary syscalls available to its process. The daemon
and other seats remain non-malicious.

**Observed opposite:** the child is not contained. Under the current process
model, loss of a bundled child’s implementation integrity is loss of FMan TCB
integrity and must be treated as compromise of the fleet.

## Axioms (trusted, not checked here)

- **A1 host process semantics:** the official Linux deployment inserts no UID,
  mount, network, PID, seccomp, or container boundary between the daemon and
  its directly spawned child. Ordinary same-UID filesystem and Unix-socket
  access semantics hold.

## Argument

### F1 (`code`) — one child can read the fleet mnemonic

`spawn_child` passes `<data-root>/seats/<seat-no>/data` as `--data-dir` but
does not change UID or enter a filesystem namespace. Production stores the
fleet database at `<data-root>/fleet-manager.sqlite`; its `identity.mnemonic`
column contains the plaintext phrase. By A1, a malicious child can resolve the
shared root from its argv path, open that database under the daemon UID, and
execute `SELECT mnemonic FROM identity WHERE id = 1`.

The same UID can also connect to `<data-root>/admin.sock`, read or modify the
wallet and sibling-seat files, and reach sibling loopback APIs. The mnemonic
derives the fleet's wallet, signing, backup, daemon, and per-seat credentials.
No further argument is needed: this concrete execution falsifies the claim.

## Residual windows

None inside this claim. The parent `wip-fman-is-secure` composition excludes
the execution from the first loss of child implementation integrity, including
the exploit-triggering step, as its accepted R4 residual; that does not make
this containment claim true.

## Weakest links

1. **F1 (`code`, fatal):** same UID plus a shared data root and admin socket
   gives the child fleet authority. Environment scrubbing is not containment.
2. The decision accepts this lack of sandboxing as a defense-in-depth residual.
   It does not relax unrelated host, operator, network, storage, or credential
   boundaries. Removing the residual would require per-seat OS isolation and
   independently validating child-derived identity before FMan signs, persists,
   publishes, or opens wallet state from it.
