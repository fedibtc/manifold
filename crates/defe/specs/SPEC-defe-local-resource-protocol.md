# SPEC-defe-local-resource-protocol: Local resource lease protocol

## Record justification

The protocol is implemented across `defe-api`, the `defe` server, and
`defe-client`, so no one of those artifacts can coherently own the contract.

Clients connect to the Unix socket named by `DEV_DEFE_SOCKET_PATH` and exchange
length-delimited CBOR requests and responses. This is a same-machine,
same-version protocol; it makes no compatibility promise between independently
upgraded releases.

A client can ping the server, allocate a resource, release a lease, or restart
a leased resource. Each successful allocation returns a handle and
resource-specific descriptor. A handle belongs only to the connection that
received it. The server rejects attempts to release or restart another
connection's handle.

The server releases every lease held by a connection when that connection
closes. In `defe exec` mode, command exit is the final resource boundary: the
server stops accepting requests and releases every remaining resource before it
returns the command's status. A restart returns the descriptor for the new
process generation while retaining the logical lease.

For a shared request, compatible live leases use one slot until its last lease
ends. An exclusive request never joins a shared slot. Errors use stable
categories so clients can distinguish invalid requests, unavailable or failed
resources, refused restarts, unknown handles, protocol decoding failures, and
server failures without matching messages.

[`../docs/rpc.md`](../docs/rpc.md) describes the wire types and operational
details.
