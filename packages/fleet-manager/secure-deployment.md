# Fleet Manager secure-deployment checklist

FMan's production-readiness property assumes this external host/operator
envelope. This repository does not inspect or certify an Umbrel, StartOS, VPS,
container runtime, or other live deployment. Record operator evidence for every
item before calling a deployment production.

- [ ] Run one active FMan for one tenant and one data root; prevent copied roots,
  overlapping upgrades, and a second process from using its identity.
- [ ] Pin the reviewed FMan artifact, including its bundled, pinned `fedimintd`
  implementation; do not replace the artifact, linked child implementation,
  libraries, entrypoint, or runtime module set.
- [ ] Restrict the data root, SQLite/WAL, seat directories, mnemonic, bitcoind
  credentials, and admin socket to the operator identity. Run any backup worker
  that reads the data root as that identity; give an external backup identity
  only its encrypted backup artifact. Give a provisioning identity only inputs
  outside the data root for the operator to import, never the data root or admin
  socket.
- [ ] Provide durable storage, free-space monitoring, and capacity response
  before writes fail. Exercise a coupled backup and restore with the mnemonic.
- [ ] Put public FI RPC behind deployment-owned abuse controls and expose the
  optional operator HTTP listener only behind its selected authenticated boundary.
- [ ] Monitor process, disk, child, relay-publication, and wallet health and
  assign response ownership for each alert.
- [ ] Preserve guardian Iroh reachability and operator network boundaries; do
  not expose local admin or data surfaces as public services.

These are operator obligations, not application startup checks. A running
process therefore does not demonstrate that this checklist holds.
