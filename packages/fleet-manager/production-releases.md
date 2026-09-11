# Production FMan releases

Production releases are prepared manually in the Umbrel and StartOS packaging
repositories. Manifold continues publishing images on master pushes or manual
workflow runs; it does not trigger platform releases.

1. Choose a successful Manifold image publish containing both amd64 and arm64.
   Use its full source commit as the image tag, not a moving branch tag.
2. In the Umbrel repository, update the production image pin, bump the app
   version, and write release notes. Publish the reviewed store update.
3. In the StartOS repository, update the production image pin to the same
   commit, bump the package version, and write release notes. Build and publish
   the signed packages through that repository's release process.

Record the exact image commit in both packages' release notes. Never replace a
released version with a different image. Keep staging apps and their data
separate. Production requires local platform-managed Bitcoin Core on mainnet.
Push notifications are optional; telemetry still registers with the collector
in the authenticated production setup-payment policy.

## Stored-data baseline

The first production release's source commit, recorded in each package's image
pin and release notes, establishes the supported persisted formats. From
that release onward, production FMan updates must preserve operator data; the
repository's disposable pre-production exception no longer applies to them.

The baseline includes the complete SQLite migration set (`0001_initial.sql`
and `0002_wallet_origin.sql` at this writing), serialized values within SQLite,
Nostr backup document version 1, wallet and guardian databases from the pinned
Fedimint release, and their data-directory layout. Git at that source commit
preserves the exact schemas, readers, serializers, and dependency pins.

Later SQLite changes add migrations; never edit an applied migration. Changes
to serialized records or backup formats need versioned readers or a migration
that preserves the baseline data. Upgrades must be tested with a populated
previous-release data root, including identity, formed seats, wallet balances,
and unfinished payouts. Test backup restoration when its format changes.
Do not publish a release requiring uninstall, reset, or re-onboarding to update.

In-place downgrades are not promised. Before an update, preserve one stopped,
complete data root containing SQLite, wallets, and guardian state. Restoring
only part of it is unsupported; Nostr recovery alone does not preserve wallet
balances. Clear restored safe-event journals before restarting a whole-volume
restore, as required by the secure-deployment contract.

The first platform update/restore qualification is release work, not proof
provided by the packaging build. Any required migration or change to platform
backup behavior needs its own reviewed scope before shipping production.
