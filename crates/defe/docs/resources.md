# Resources

A resource is anything the server can allocate, share, release, and clean up.
Supported resources are `NostrRelay`, `PushGateway`, `Bitcoind`, `Fman`, `Flip`,
and `Gatewayd`.


## Handles

Every allocation returns a `ResourceHandleId` scoped to the client connection that requested it.

Rules:

- A handle is valid only on the connection that received it.
- `Release(handle_id)` drops that lease early.
- Connection drop releases every remaining handle owned by that connection.
- Server shutdown releases every resource regardless of owner.


## Shared versus exclusive

Exclusive resources:

- Always create a new resource slot.
- Are owned only through the requesting connection's handle.
- Are never inserted into the shared pool.

Shared resources:

- Are created lazily on first compatible request.
- Are reused by later compatible requests while at least one lease is alive.
- Are stored in a global shared map by `SharedResourceKey` only while alive.
- Are terminated and removed when the last lease is released or its connection drops.
- A later compatible request starts a new shared resource if the previous one was already released.

Nostr relay, push gateway, and bitcoind use singleton shared keys. FMan, FLIP,
and gatewayd keys include their complete launch requests, so only identical
requests share a slot:

```rust
SharedResourceKey::NostrRelay
SharedResourceKey::PushGateway
SharedResourceKey::Bitcoind
SharedResourceKey::Fman(request)
SharedResourceKey::Flip(request)
SharedResourceKey::Gatewayd(request)
```

New resource kinds should derive keys from configuration where a singleton is
not sufficient, for example:

```rust
SharedResourceKey::Foo { profile: FooProfile, size: u16 }
```


## Resource slots

Represent every live or restartable resource as a slot. A slot separates stable allocation from process state.

Stable allocation:

- resource id
- resource kind
- sharing key if shared
- requested config
- port allocations
- data directory
- config path
- log path
- latest descriptor

Process state:

- running child process
- exited status
- stopped state
- startup failure state if useful

This layout lets a client restart an exited process without reallocating the entire logical resource.


## Restart behavior

The server does not automatically restart a failed process.

Client-requested restart modes:

- `IfExited`: restart only if the process already exited. Return an error if it is running.
- `Force`: terminate the current process if running, then start it again.

Restart should preserve stable allocation when possible. For Nostr relay, preserve data directory, config path, and port. For push gateway, preserve port, app id, and SQLite database path under `resources/push-gateway/<slot>/push-gateway.sqlite`. For bitcoind, preserve RPC/P2P ports, RPC credentials, and data directory.

Forced restart of a shared resource affects all clients sharing the same slot. This is acceptable for dev tooling.


## Cleanup order

On explicit release:

1. Remove the handle from the connection state.
2. Drop that lease.
3. If no leases remain, remove any shared-map entry for the slot, terminate the process, and remove the slot.

On connection drop:

1. Drop every handle owned by that connection.
2. Apply the same cleanup as explicit release for each handle.

On server finalization:

1. Stop accepting new requests.
2. Drop all connection states.
3. Drop the shared map.
4. Terminate and wait for all children.


## Concurrency

The resource manager serializes resource-map changes, including start, restart,
and stop, under its global mutex. This makes concurrent requests for the same
shared key coalesce to one slot. The manager currently holds that mutex while it
starts or stops a process; replacing this with per-slot transition state must
preserve that coalescing and prevent handles from observing a partial restart.
