# Proof maintenance: Fleet Manager confines secret-dependent content

This companion records the authorized scope change for
[CLAIM-fleet-manager-confines-secret-dependent-content](../CLAIM-fleet-manager-confines-secret-dependent-content.md).
It is not a fresh proof of the broad non-flow property, so the claim remains
**Unverified**.

## Scope and model

The narrow recheck read the pinned Fedimint source and its local Nix patch,
FMan's release entrypoint and parser, and the FMan Nostr-backup format. It
grants the claim's cryptographic assumptions. It does not treat a signature,
hash, or ciphertext in another channel as a public projection.

## Authorized exceptions and repairs

1. **Mint debug fingerprint (`assumption`).** Pinned
   `DerivableSecret::Debug` emits exactly eight bytes derived through HKDF-SHA512
   under a fixed domain-separation string. The owner accepts that one
   computationally one-way projection under the stated high-entropy-secret
   model. It is not eligible for `safe_to_share`: the safe-event policy has the
   stricter rule that all secret-derived fingerprints remain unsafe to share.
2. **Lightning payment retry (`code`).** The Nix-applied patch changes the two
   `fedimint-ln-client::pay` warnings at their logging source. The old warnings
   formatted `PayInvoicePayload`, gateway metadata, and remote error text. The
   new warnings format only fixed messages, a numeric HTTP error code where
   present, and retry durations. Thus a gateway cannot make those warnings echo
   payment authentication or invoice metadata.
3. **Nostr backup (`assumption`, `test`).** The named kind-37708 format seals
   payloads with XChaCha20-Poly1305 and carries only its HMAC-blinded coordinate
   in the `d` tag. `fman-nostr` format tests pin the sole-coordinate shape,
   cross-mnemonic blinding, encryption round trip, wrong-mnemonic rejection, and
   fixed-size padding. The exception is restricted to that authenticated,
   signed backup protocol.
4. **Bitcoind password (`test`).** The packaged entrypoint now emits one
   `--bitcoind-password=<value>` token. The package validation runs that
   entrypoint with a leading-hyphen dummy password and captures the exact token;
   the FMan parser test independently accepts the same equals-form invocation.

## Boundary

The entrypoint still receives the platform password and the daemon passes it to
its bundled child through the documented single-tenant host boundary. Replacing
that child contract with a secret file or descriptor would change the FMan,
Fedimint, and platform interfaces and is deliberately outside this repair.
The equals form prevents clap from reflecting a leading-hyphen value on stderr;
it does not claim that command-line arguments are secret storage.

## Residuals

This maintenance does not establish that every current or future FMan content
exit is safe. New formatted values, child output, remote error text, or
`safe_to_share` events require the normal claim and tracing audits.
