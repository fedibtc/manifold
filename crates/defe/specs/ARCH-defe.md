# ARCH-defe: Local test resource runner

`defe` gives integration and E2E tests one local owner for short-lived external
resources. It consists of the `defe` server, the shared `defe-api` protocol
crate, `defe-client` (including `defe-cli`), and `defe-portalloc`.

Tests normally run their command under `defe exec`. The server creates a private
temporary root and Unix socket, exports `DEV_DEFE_SOCKET_PATH` to the command,
and owns every resource requested through that socket. `defe serve --listenfd`
instead accepts an inherited Unix listener for a persistent development server.
Both modes keep the server and its resources local to the developer or CI job.

The server supervises resource processes and owns their resource directories,
ports, logs, and stable slot state. It provides local Nostr relays, push
gateways, Bitcoin Core regtest nodes, Fleet Managers, FLIP daemons, and Fedimint
gateway daemons. `defe-portalloc` coordinates temporary loopback port
reservations across separate local processes; operating-system bindings remain
the authority once a resource starts.

Clients receive connection-scoped leases through the
[SPEC-defe-local-resource-protocol](SPEC-defe-local-resource-protocol.md).
Releasing a lease ends that client's ownership; dropping its connection or
ending the `defe exec` command releases all of its leases. A shared slot exists
only while it has leases; an exclusive request always has its own slot. Resource
restarts preserve a slot's stable allocation while replacing its process
generation.

Read [`../SECURITY.md`](../SECURITY.md) before changing resource process
spawning, socket handling, temporary paths, or resource descriptors. Detailed
operator and implementation guidance lives in [`../docs/`](../docs/).
