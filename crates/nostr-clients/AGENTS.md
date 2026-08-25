# Nostr clients crate instructions

- Keep this crate as the component-level Nostr boundary.
- Read [`./specs/ARCH-nostr-clients.md`](./specs/ARCH-nostr-clients.md) before changing crate boundaries or public APIs.
- Read `./SECURITY.md` before changing publish, fetch, verification, or relay-error behavior.
- Do not add public generic Nostr publish/fetch escape hatches unless the API is explicitly reviewed as a low-level boundary.
- Treat all fetched Nostr events as untrusted indexing candidates. Callers must verify signatures, content, tag consistency, credential proofs, and revocation state before using them.
- Protocol constants, tag construction helpers, and cross-program document schemas and their signing/verification belong in `crates/nostr`; relay I/O and `nostr-sdk` usage belong here.
