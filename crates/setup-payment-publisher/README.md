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

The JSON file is the shared wire object, not a tool-specific schema. Rebuild
after protocol changes; the golden serialization test forces additions or
serialized defaults to be reflected in operator policy files, while old
binaries reject fields they do not know.

## Testing

Pure tests use fake relay operations and never contact Production. The ignored
real-relay test uses an exclusive local `defe` lease; see [`testing.md`](./testing.md).
