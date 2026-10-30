# Setup-payment publisher

This production-only tool signs and publishes the complete kind-37707 policy
defined by [`SPEC-setup-payment-federations`](../../specs/SPEC-setup-payment-federations.md).
Copy `example-policy.json` and replace every placeholder; the committed template
is deliberately invalid so it cannot be published unchanged.

Obtain `--expected-publisher` independently, out of band from the secret-key
source. Build the reproducible tool with `nix build .#setup-payment-publisher`.
The secret is never accepted through argv or an environment variable.

For the first publication, after confirming out of band that no earlier receipt
or publication exists:

```console
password-manager read tsp-publisher-key | \
  nix run .#setup-payment-publisher -- publish \
    --content policy.json \
    --expected-publisher <hex-or-npub> \
    --first-publication \
    --receipt signed-event.json
```

For every update, supply the latest retained receipt:

```console
password-manager read tsp-publisher-key | \
  nix run .#setup-payment-publisher -- publish \
    --content policy.json \
    --expected-publisher <hex-or-npub> \
    --previous-receipt previous-signed-event.json \
    --receipt new-signed-event.json
```

The custodian must retain the newest receipt as the publisher high-water mark.
The tool checks it against every Production relay before loading the secret.
Publishing an empty stop-set additionally requires `--allow-empty-stop-set`.
Exactly one publish or republish operation for this key may run at a time across
all custodians and hosts; the custody procedure must serialize operations and
retain the winning newest receipt. Relay preflight is not a distributed lock.

If one relay fails after the receipt is saved, retry the same signed event
without the secret:

```console
nix run .#setup-payment-publisher -- republish \
  --receipt signed-event.json \
  --expected-publisher <hex-or-npub>
```

To let MGs start direct-proof guardian transfers, set
`"guardian_transfer_v2": true` in the policy file; set `false` to stop new
transfers. If you omit the flag, each environment's default applies: on in
development and staging, off in production. MGs continue transfers they
already stored regardless of the flag.

The retired `guardian_transfer` field must never be set to `true` in production.
MG 0.1.9 and older FA transfer readers use it to enable the incompatible old
`TransferSeat` shape; they ignore the new flag and stay off in production
when the retired field is absent. This publisher no longer accepts the
retired field.

Before enabling the new flag in production, upgrade both source and destination
MGs and set `min_mg_version` to the first release with the new shape. MGs
that understand this minimum stop advertising new seats when below it;
older readers may still advertise, so endpoint upgrades remain required.
The minimum version is not a protocol
handshake and does not recover existing mixed-version transfers. To recover
those transfers, upgrade both endpoints. See
[Enabling transfer](../mg/specs/SPEC-guardian-transfer.md#enabling-transfer).

To stop MGs below a release from advertising new seats, for versions that
understand the minimum, set
`"min_mg_version": "0.1.9"` (a SemVer release no newer than `fman_version`).
Those MGs keep running their seats and show that an update is needed; their
uptime is not affected. Omitted, there is no minimum.

The JSON file is the shared wire object, not a tool-specific schema. Rebuild
after protocol changes; the golden serialization test forces additions or
serialized defaults to be reflected in operator policy files. Older consumers
that admit unknown fields ignore the new flag; only the initial release
rejects fields it does not know.

## Testing

Pure tests use fake relay operations and never contact Production. The ignored
real-relay test uses an exclusive local `defe` lease; see [`testing.md`](./testing.md).
