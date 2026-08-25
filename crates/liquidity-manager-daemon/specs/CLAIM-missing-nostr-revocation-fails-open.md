# CLAIM-missing-nostr-revocation-fails-open: Missing nostr revocation fails open

FLIP cannot insert a new allocation for a request when a credential it must
freshly check comes from an issuer authority with no `nostr` revocation
location in the immutable authority snapshot for that verification pass.

The adversary is a hostile FI holding an otherwise valid FMan endorsement and
an FMan credential that its issuer has revoked. It cannot forge an issuer,
change FLIP's installed issuer authority, or use Admin verbs. The
issuer-signed authority in the verification pass's immutable snapshot lists
only a non-Nostr location, such as HTTPS. The FI can keep the pre-revocation
credential and live FMan advertisement available. The claim is about FLIP's
advertised Nostr fresh-revocation boundary, not a guarantee that every issuer
supports Nostr.

## Status

Unverified.

## Assumptions

None. The claim is about FLIP's local control flow after it sees a credential
from an installed authority without a Nostr location; it does not rely on the
meaning or cryptographic validity of a revocation outside that local domain.
