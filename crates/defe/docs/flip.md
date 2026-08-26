# FLIP daemon resource

`Flip` is an exclusive local `liquidity-manager-daemon` resource. It owns the
daemon process only; gatewayd remains a distinct service owned by the test
harness, as FLIP does not own gatewayd in production.

`defe-api` exposes:

```rust
ResourceRequest::Flip(FlipRequest {
    iroh_connect_overrides,
    holder_authorization_relay_url,
})
ResourceDescriptor::Flip(FlipInfo { .. })
```

The request may supply a direct Iroh route map for a locally formed federation
and a local relay to pin Holder-authorization reconciliation to.
Defe chooses the data directory, admin and public ports, bootstrap token,
provider identity, trust-fixture directory, and log path. The descriptor
returns the admin URL/token, stable data and fixture directories, and provider
public key. It never returns the provider secret.

The daemon starts without federation setup. The consumer writes any
invite-dependent trust fixtures, installs provider trust, and applies the
gateway/relay/Bitcoin setup through the returned Admin API after it has the
target federation invite.

FLIP may be shared when a caller deliberately wants one identical daemon
configuration. Tests that mutate Admin setup, trust fixtures, or allocation
state use exclusive leases. Restart keeps the same slot configuration and data
directory while starting a new process generation.
