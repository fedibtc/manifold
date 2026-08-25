# Fleet Manager agent notes

The FMan is one component across its sibling crates. Its `specs/` and
`claims/` records govern all of them, which is why they live here rather than
in any one crate. Read `specs/ARCH-fleet-manager.md` first.

- `core` — the FMan itself: seats, storage, allocation, identity derivation,
  guardian-fee policy, and the FI/operator surfaces.
- `cli` — the operator admin command tree and local admin-socket client.
- `fedimint`, `nostr` — implementations of the capabilities `core` defines.
  They depend on `core`; `core` depends on neither.
- `bin` — the daemon composition root, and the only crate that names all
  capability implementations.

Rules that keep that shape:

- Capabilities are trait-shaped holes `core` defines and does not fill —
  `wallet::EcashWallet`, `guardian_fee::GuardianFeeVault`,
  `backup::{BackupSink, BackupArchive}`. Adding a capability adds a dependency to `bin`, never to `core`. A new
  dependency in `core`'s `Cargo.toml` is a change worth justifying in review.
- A trait in `core` is *exactly* a hole: it exists when the implementor lives
  above `core`. What `fedimint` and `nostr` read out of the daemon is called
  concretely, because they depend on `core` and can name its types. Before
  adding a trait, name its production implementor; if that is `core`, delete
  the trait. Test doubles as the only other implementors are the tell.
- A hole is something the daemon needs *done*. What it needs to *know* — the
  runtime's latest observation, like `directory::DirectoryPresence` — is a
  value on a watch channel, not a trait to call back into. That makes "the
  admin socket never waits on a relay" true by construction.
- Vocabulary the store persists or the policy prices in lives in `core`, above
  any implementation. If a type has to be imported from an adapter crate to
  write a row or compare a price, it is in the wrong crate.
- The critical/simple split is deliberate: what decides where money goes is in
  `core`, what moves it is behind a hole. Keep it that way when extending
  either.
- `fedimint/SECURITY.md` and the root `SECURITY.md` bound the payment and
  credential paths; read them before touching wallet, credential, or
  network-facing code.
