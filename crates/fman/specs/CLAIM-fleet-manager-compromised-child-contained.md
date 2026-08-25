# CLAIM-fleet-manager-compromised-child-contained: A compromised guardian child is contained

After an officially spawned seat `fedimintd` becomes arbitrarily malicious,
it cannot read or modify fleet-level assets: the root mnemonic or SQLite
database, wallet state, the admin socket, or another seat's data and authority.

The child may issue arbitrary syscalls available to its process. The daemon
and other seats remain non-malicious.

## Status

Falsified: a same-UID child can read the fleet database, reach the admin socket, and access other fleet authority paths.
The accepted result is a TCB boundary, not sandbox containment.

## Assumptions

- **A1 host process semantics:** the official Linux deployment inserts no UID,
  mount, network, PID, seccomp, or container boundary between the daemon and
  its directly spawned child. Ordinary same-UID filesystem and Unix-socket
  access semantics hold.
