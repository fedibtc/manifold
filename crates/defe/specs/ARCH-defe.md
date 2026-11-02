# ARCH-defe: Local test resource runner

`defe` gives integration and E2E tests one local owner for short-lived external
resources. It consists of the `defe` server, the shared `defe-api` protocol
crate, `defe-client` (including `defe-cli`), `defe-portalloc`, and the
`defe-env` foreground environment composer.

Tests normally run their command under `defe exec`. The server creates a private
temporary root and Unix socket, exports `DEV_DEFE_SOCKET_PATH` to the command,
and owns every resource requested through that socket. `defe serve --listenfd`
instead accepts an inherited Unix listener for a persistent development server.
Both modes keep the server and its resources local to the developer or CI job.
`defe env` uses the same one-shot server boundary while its composer holds
the leases needed to form a federation, connect a gateway, and advertise FLIP.
After readiness it launches an explicit command or `$SHELL` with generated
cross-shell tools and stable `DEFE_ENV_*` discovery paths. That child's lifetime
is the environment lifetime. The multithreaded Linux composer remains outside
the broker's user and PID namespaces; status proxies place every setup and
command subprocess inside the PID namespace across interactive job-control
groups and daemonization. Destroying and reaping namespace init is
the kernel-owned completion boundary before lease release. The child's exit
status crosses both composer and server boundaries unchanged.

Generated environment tools such as `fees` and `traffic` exercise local
plumbing only; none models production Fedi payer-fee accrual
([`../docs/cli.md`](../docs/cli.md)). The environment serializes its owned
`fa-cli` calls because the CLI is a single-developer test tool rather than a
concurrent consumer
([GATE-fa-cli-test-tool-scope](../../fa-cli/specs/GATE-fa-cli-test-tool-scope.md)).

The server supervises resource processes and owns their resource directories,
ports, logs, and stable slot state. It provides local Nostr relays, push
gateways, Bitcoin Core regtest nodes, Manifold Guardians, FLIP daemons, and Fedimint
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
