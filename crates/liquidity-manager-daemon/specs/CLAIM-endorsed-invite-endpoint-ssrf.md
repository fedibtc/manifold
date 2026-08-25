# CLAIM-endorsed-invite-endpoint-ssrf: Endorsed invite endpoint ssrf

A hostile FI holding an otherwise valid FMan endorsement cannot make FLIP's
production verification path initiate a network connection to an arbitrary
host/port chosen only by that FI before FLIP has authenticated the target
federation configuration.

The FI holds a valid, unrevoked endorsement for a federation id, has a ready
provider, and can make a valid self-signed public RPC request with its
recomputed details hash. Its fresh endorsement-revocation lookup completes. It
cannot forge the FMan's attestation or credential, change configured
issuers/relays, or use Admin. It may construct any syntactically valid
Fedimint invite code, including one that retains the endorsed federation id
while replacing its API URL with a loopback, link-local, or operator-private
URL. The property covers outbound connection attempts, not acceptance of a
federation or reading any particular internal response.

## Status

Falsified: the endorsement authenticates only the federation id; FLIP dials the
attacker-chosen API URL in the invite before authenticating the federation
configuration returned by that endpoint.

## Assumptions

- **A1 — network observability.** Attempting a protocol connection to an
  attacker-selected reachable host/port is a security-relevant outbound effect
  even if the peer does not return a valid Fedimint response.
- **A2 — endpoint reachability.** FLIP can have network reachability to hosts
  its FI cannot reach, including loopback or deployment-private services.
