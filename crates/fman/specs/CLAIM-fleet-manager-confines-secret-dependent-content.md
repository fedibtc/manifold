# CLAIM-fleet-manager-confines-secret-dependent-content: Fleet Manager confines secret-dependent content

The serving Fleet Manager daemon does not format or serialize content whose
bytes could change if a daemon-held secret's bytes changed while every untainted
input stayed fixed, into its logs or stderr, iroh RPC traffic, Nostr
publications, outbound push-gateway requests, or stdout.

Daemon-held secrets are the root mnemonic and derived private keys; seat API
authorization; ecash spend, blinding, issuance, and refund authority; the
bitcoind password; payment-federation API bearer secrets; and DKG-completion
callback bearer and idempotency secrets. The closed list of permitted public
projections is amounts, counts, federation IDs, signatures, public keys,
node/endpoint IDs derived from public keys, iroh handshake transcripts,
blinded issuance requests, and the exact eight-byte HKDF debug fingerprint
emitted by pinned Fedimint's `DerivableSecret` formatter. It also includes only
the dedicated Nostr-backup format's signed kind-37708 events: their
XChaCha20-Poly1305 ciphertext and the HMAC-SHA256-blinded coordinate used as
the event's sole `d` tag. The daemon may also send zero or more
DKG-completion callback retries: POST to the exact deployment-pinned origin and
`/hooks/{hook_id}/{hook_secret}` bearer path, with the stable idempotency key in
an `InvokeHookRequest` whose `data` is empty.

This property covers emitted payload bytes, not secret-dependent occurrence,
selection, count, order, timing, or interleaving of fixed-content emissions. It
excludes files under the data root and owner-only admin-socket traffic, loopback
child traffic, host process state and memory, operator-supplied public
configuration that duplicates a secret, and separate CLI or directly invoked
`fedimintd` processes.

## Status

Unverified.


## Assumptions

- The host is single-tenant. Files under the data root, loopback traffic,
  process state, core dumps, child processes, and the owner-only admin socket
  remain inside its trust boundary; logs do not.
- Bundled `fedimintd` output, response bodies, rejection details, and observable
  process status do not depend on its secret environment, request authorization,
  or generated secret material except through the listed public projections.
- The exact eight-byte `DerivableSecret` debug fingerprint is an HKDF-SHA512
  output under a fixed domain-separation string from a high-entropy
  mnemonic-derived secret. It is computationally one-way under the stated
  cryptographic model. This narrow exception does not make arbitrary secret
  fingerprints, hashes, or debug output public.
- For the dedicated backup format only, HMAC-SHA256 under the independently
  derived tag key is unforgeable and hides its seat-id input, XChaCha20-Poly1305
  under the independently derived encryption key provides confidentiality and
  ciphertext integrity, and the backup identity's event signature authenticates
  kind-37708 publications. Encryption, signing, or hashing elsewhere does not
  remove taint.
- HTTPS authenticates the configured gateway and protects request content;
  development HTTP targets are restricted to numeric loopback addresses.
