# CLAIM-fleet-manager-preserves-published-guardian-data: Published guardian data is not deleted

After any successful FI-facing `GetInviteCode` or `GetStatus` response carries a
federation invite, no later official-daemon execution deletes that seat's
`fedimintd` data directory.

## Status

Unverified.

## Assumptions

- Startup state has a valid history produced by the official implementation,
  including an official restore when applicable. No process other than the
  official daemon and its officially launched `fedimintd`, and no operator
  action, mutates the data root;
  the data root lock excludes another daemon, and committed SQLite writes
  survive crashes and reload faithfully.
- `fedimintd` never removes its own data directory. Reaching consensus through
  the official lifecycle requires the current attempt's guardian code to be
  durable, and restart stops and reaps the previous child before appending a
  fresh codeless attempt.
- Tokio cancellation, filesystem deletion, child stop and reap, `kill_on_drop`,
  and parent-death termination follow their documented behavior.
