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
is the environment lifetime; its exit status crosses both composer and server
boundaries unchanged.

The generated `fees` tool exposes FMan's real fee show and collection commands.
Its `synthetic-remit` preparation action creates a metadata-bearing stability-pool
deposit through a dedicated ordinary `fi-cli` payment wallet, then waits for the
selected FMan to observe it. It exists to exercise the local remittance and
collection plumbing: it directly selects a recipient and amount and therefore
does not model production Fedi payer accrual, share splitting, accumulation, or
scheduling. The environment serializes its owned `fi-cli` calls, including that
wallet, because the CLI is a single-developer test tool rather than a concurrent
consumer ([GATE-fi-cli-test-tool-scope](../../fi-cli/specs/GATE-fi-cli-test-tool-scope.md)).

The generated `traffic` tool wraps the flake-pinned Fedimint load tester.
Connection traffic repeatedly downloads client configuration through real client
API connections. The private generated wrapper supplies the environment's invite
and Iroh routes to the selected trusted tool, serializes calls, and bounds their
load and lifetime. Modes whose required upstream capability or environment
dependency is unavailable fail explicitly rather than simulating success. None
of these ordinary operations models or causes production Fedi payer-fee accrual.

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
